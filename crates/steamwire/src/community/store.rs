//! The Steam store: the account's cart (`IAccountCartService`) and buying
//! it with the Steam Wallet through the checkout, as the store's own
//! `checkout.js` does.
//!
//! Only the wallet can pay: the checkout is always started with the
//! `steamaccount` payment method, no card or bank field is ever sent, and a
//! quote Steam would charge to anything else is refused before it can be
//! finalized. Nothing is charged until [`Store::finalize`], which is never
//! retried. Free games don't need the checkout:
//! `SteamClient::request_free_license` adds them.

use std::time::Duration;

use crate::enums::{EPaymentMethod, EPurchaseResultDetail};
use crate::EResult;
use serde::Deserialize;

use crate::community::error::{Error, Result};
use crate::community::session::{json, WebSession};
use crate::community::util::{flexible_bool, optional_flexible_u64};

/// How often, and how many times, the outcome of a purchase is asked for
/// while Steam is still processing it (as `checkout.js`).
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const POLL_ATTEMPTS: usize = 60;

mod cart {
    //! `IAccountCartService` messages (Steam's `service_accountcart.proto`),
    //! only the fields used here.

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct CartAmount {
        #[prost(int64, optional, tag = "1")]
        pub amount_in_cents: Option<i64>,
        #[prost(int32, optional, tag = "2")]
        pub currency_code: Option<i32>,
        #[prost(string, optional, tag = "3")]
        pub formatted_amount: Option<String>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct LineItem {
        #[prost(uint64, optional, tag = "1")]
        pub line_item_id: Option<u64>,
        #[prost(uint32, optional, tag = "3")]
        pub packageid: Option<u32>,
        #[prost(uint32, optional, tag = "4")]
        pub bundleid: Option<u32>,
        #[prost(bool, optional, tag = "8")]
        pub is_valid: Option<bool>,
        #[prost(message, optional, tag = "11")]
        pub price_when_added: Option<CartAmount>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Contents {
        #[prost(message, repeated, tag = "1")]
        pub line_items: Vec<LineItem>,
        #[prost(message, optional, tag = "2")]
        pub subtotal: Option<CartAmount>,
        #[prost(bool, optional, tag = "3")]
        pub is_valid: Option<bool>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ItemToAdd {
        #[prost(uint32, optional, tag = "1")]
        pub packageid: Option<u32>,
        #[prost(uint32, optional, tag = "2")]
        pub bundleid: Option<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct AddItemsRequest {
        #[prost(string, optional, tag = "1")]
        pub user_country: Option<String>,
        #[prost(message, repeated, tag = "2")]
        pub items: Vec<ItemToAdd>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct AddItemsResponse {
        #[prost(uint64, repeated, packed = "false", tag = "1")]
        pub line_item_ids: Vec<u64>,
        #[prost(message, optional, tag = "2")]
        pub cart: Option<Contents>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct GetCartRequest {
        #[prost(string, optional, tag = "1")]
        pub user_country: Option<String>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct CartResponse {
        #[prost(message, optional, tag = "1")]
        pub cart: Option<Contents>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RemoveItemRequest {
        #[prost(uint64, optional, tag = "1")]
        pub line_item_id: Option<u64>,
        #[prost(string, optional, tag = "2")]
        pub user_country: Option<String>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Empty {}
}

/// An amount of money in the account's currency.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Amount {
    /// In cents (the currency's minor unit).
    pub cents: i64,
    /// Steam's `ECurrencyCode`.
    pub currency: i32,
    /// As the store shows it, such as "13,29€".
    pub formatted: String,
}

impl From<cart::CartAmount> for Amount {
    fn from(amount: cart::CartAmount) -> Self {
        Self {
            cents: amount.amount_in_cents.unwrap_or_default(),
            currency: amount.currency_code.unwrap_or_default(),
            formatted: amount.formatted_amount.unwrap_or_default(),
        }
    }
}

/// A line of the cart.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CartItem {
    /// The line's ID, for [`Store::remove_from_cart`].
    pub line_item_id: u64,
    /// The package, if it is one.
    pub package_id: Option<u32>,
    /// The bundle, if it is one.
    pub bundle_id: Option<u32>,
    /// Steam can sell it to this account.
    pub is_valid: bool,
    /// Its price when it was added.
    pub price: Option<Amount>,
}

/// The account's cart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Cart {
    /// Its lines.
    pub items: Vec<CartItem>,
    /// The sum.
    pub subtotal: Option<Amount>,
    /// It can be checked out.
    pub is_valid: bool,
}

impl From<Option<cart::Contents>> for Cart {
    fn from(contents: Option<cart::Contents>) -> Self {
        let Some(contents) = contents else { return Self::default() };
        Self {
            items: contents
                .line_items
                .into_iter()
                .map(|item| CartItem {
                    line_item_id: item.line_item_id.unwrap_or_default(),
                    package_id: item.packageid.filter(|&id| id != 0),
                    bundle_id: item.bundleid.filter(|&id| id != 0),
                    is_valid: item.is_valid.unwrap_or_default(),
                    price: item.price_when_added.map(Amount::from),
                })
                .collect(),
            subtotal: contents.subtotal.map(Amount::from),
            is_valid: contents.is_valid.unwrap_or_default(),
        }
    }
}

/// What to put in the cart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartEntry {
    /// A package (a "sub"), as the store page's purchase box sells it.
    Package(u32),
    /// A bundle.
    Bundle(u32),
}

#[derive(Deserialize)]
struct InitResponse {
    success: i32,
    /// A number or a string, `-1` without a transaction.
    #[serde(default)]
    transid: Option<serde_json::Value>,
    #[serde(default)]
    paymentmethod: Option<i32>,
    #[serde(default)]
    purchaseresultdetail: Option<i32>,
}

#[derive(Deserialize)]
struct FinalPriceResponse {
    success: i32,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default, rename = "steamAccountBalance")]
    steam_account_balance: Option<i64>,
    #[serde(default, rename = "providerpaymentmethod")]
    provider_payment_method: Option<i32>,
    #[serde(default, rename = "formattedTotal")]
    formatted_total: Option<String>,
    #[serde(default, rename = "requirecvv", deserialize_with = "flexible_bool")]
    require_cvv: bool,
    #[serde(default)]
    purchaseresultdetail: Option<i32>,
}

#[derive(Deserialize)]
struct StatusResponse {
    success: i32,
    #[serde(default)]
    purchaseresultdetail: Option<i32>,
    #[serde(default)]
    purchasereceipt: Option<Receipt>,
}

/// The receipt of a purchase.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Receipt {
    /// The transaction's ID.
    #[serde(default, rename = "transactionid", deserialize_with = "optional_flexible_u64")]
    pub transaction_id: Option<u64>,
    /// What was paid, as the store shows it.
    #[serde(default, rename = "formattedTotal")]
    pub formatted_total: Option<String>,
    /// How it was paid (`EPaymentMethod`).
    #[serde(default, rename = "paymentmethod")]
    pub payment_method: Option<i32>,
}

/// A transaction Steam has priced, waiting to be [finalized](Store::finalize)
/// or [cancelled](Store::cancel). Nothing has been charged.
#[derive(Debug)]
#[must_use = "a quote holds a pending transaction: finalize or cancel it"]
#[non_exhaustive]
pub struct Quote {
    /// The transaction's ID.
    pub transaction_id: u64,
    /// What Steam will charge, in cents.
    pub total_cents: i64,
    /// As the store shows it.
    pub formatted_total: Option<String>,
    /// The wallet's balance, if Steam said.
    pub wallet_balance_cents: Option<i64>,
}

/// How a finalized purchase ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PurchaseOutcome {
    /// Paid; the receipt, if Steam sent one.
    Completed(Option<Receipt>),
    /// Steam accepted it but will finish it later; it shows in the purchase
    /// history meanwhile.
    Pending,
}

/// The store and checkout, for the account a [`WebSession`] is logged in to.
#[derive(Clone, Debug)]
pub struct Store {
    session: WebSession,
    country: Option<String>,
}

impl Store {
    /// Uses `session`.
    pub fn new(session: WebSession) -> Self {
        Self { session, country: None }
    }

    /// Prices the cart for `country` (two letters, such as "FI") instead of
    /// the account's.
    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    /// The account's cart (`IAccountCartService/GetCart`).
    pub async fn cart(&self) -> Result<Cart> {
        let request = cart::GetCartRequest { user_country: self.country.clone() };
        let response: cart::CartResponse =
            self.session.api_protobuf(false, "IAccountCartService", "GetCart", 1, &request).await?;
        Ok(response.cart.into())
    }

    /// Adds `entries` to the cart (`IAccountCartService/AddItemsToCart`).
    pub async fn add_to_cart(&self, entries: &[CartEntry]) -> Result<Cart> {
        let request = cart::AddItemsRequest {
            user_country: self.country.clone(),
            items: entries
                .iter()
                .map(|entry| match *entry {
                    CartEntry::Package(id) => cart::ItemToAdd { packageid: Some(id), bundleid: None },
                    CartEntry::Bundle(id) => cart::ItemToAdd { packageid: None, bundleid: Some(id) },
                })
                .collect(),
        };
        let response: cart::AddItemsResponse =
            self.session.api_protobuf(true, "IAccountCartService", "AddItemsToCart", 1, &request).await?;
        Ok(response.cart.into())
    }

    /// Removes a line from the cart (`IAccountCartService/RemoveItemFromCart`).
    pub async fn remove_from_cart(&self, line_item_id: u64) -> Result<Cart> {
        let request = cart::RemoveItemRequest { line_item_id: Some(line_item_id), user_country: self.country.clone() };
        let response: cart::CartResponse =
            self.session.api_protobuf(true, "IAccountCartService", "RemoveItemFromCart", 1, &request).await?;
        Ok(response.cart.into())
    }

    /// Empties the cart (`IAccountCartService/DeleteCart`).
    pub async fn clear_cart(&self) -> Result<()> {
        let _: cart::Empty =
            self.session.api_protobuf(true, "IAccountCartService", "DeleteCart", 1, &cart::Empty {}).await?;
        Ok(())
    }

    /// Starts buying the cart with the wallet and asks Steam for the final
    /// price (`checkout/inittransaction` and `checkout/getfinalprice`).
    /// Nothing is charged; the quote is cancelled again if the wallet can't
    /// pay all of it.
    pub async fn quote(&self) -> Result<Quote> {
        let checkout = &self.session.urls().checkout;
        let referer = format!("{checkout}/checkout/?accountcart=1");
        let init: InitResponse = json(
            self.session
                .post(
                    &format!("{checkout}/checkout/inittransaction/"),
                    &wallet_form(self.session.session_id()),
                    Some(&referer),
                )
                .await?,
        )
        .await?;
        let transid = init.transid.as_ref().and_then(|t| t.as_u64().or_else(|| t.as_str()?.parse().ok()));
        let transaction_id = match transid {
            Some(id) if init.success == 1 => id,
            _ => return Err(purchase_error(init.success, init.purchaseresultdetail)),
        };
        if let Some(method) = init.paymentmethod.map(EPaymentMethod).filter(|&m| !is_wallet_or_undecided(m)) {
            self.cancel_transaction(transaction_id).await;
            return Err(Error::PurchaseRefused(format!("Steam chose payment method {method:?} instead of the wallet")));
        }

        let query = [
            ("count", "1".to_string()),
            ("transid", transaction_id.to_string()),
            ("purchasetype", "self".into()),
            ("microtxnid", "-1".into()),
            ("cart", "-1".into()),
            ("gidReplayOfTransID", "-1".into()),
        ];
        let price: Result<FinalPriceResponse> =
            async { json(self.session.get(&format!("{checkout}/checkout/getfinalprice/"), &query).await?).await }.await;
        let refusal = match &price {
            Err(_) => None,
            Ok(price) if price.success != 1 => Some(purchase_error(price.success, price.purchaseresultdetail)),
            Ok(price) => wallet_refusal(price).map(Error::PurchaseRefused),
        };
        let price = match (price, refusal) {
            (Ok(price), None) => price,
            (Err(e), _) | (Ok(_), Some(e)) => {
                self.cancel_transaction(transaction_id).await;
                return Err(e);
            }
        };
        Ok(Quote {
            transaction_id,
            total_cents: price.total.unwrap_or_default(),
            formatted_total: price.formatted_total,
            wallet_balance_cents: price.steam_account_balance,
        })
    }

    /// Cancels `quote`'s transaction (`checkout/canceltransaction`).
    pub async fn cancel(&self, quote: Quote) -> Result<()> {
        let url = format!("{}/checkout/canceltransaction/", self.session.urls().checkout);
        let response = self.session.post(&url, &[("transid", quote.transaction_id.to_string())], None).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::HttpStatus(response.status().as_u16()))
        }
    }

    async fn cancel_transaction(&self, transaction_id: u64) {
        let url = format!("{}/checkout/canceltransaction/", self.session.urls().checkout);
        if let Err(e) = self.session.post(&url, &[("transid", transaction_id.to_string())], None).await {
            tracing::warn!("cancelling transaction {transaction_id} failed: {e}");
        }
    }

    /// **Pays** `quote` from the wallet (`checkout/finalizetransaction`), if
    /// its total is at most `max_total_cents`, and waits for the outcome.
    ///
    /// Never retried: if the request or the wait fails after it was sent,
    /// the result is [`Error::PurchaseUnconfirmed`], and the purchase
    /// history tells whether it went through.
    pub async fn finalize(&self, quote: Quote, max_total_cents: i64) -> Result<PurchaseOutcome> {
        if quote.total_cents > max_total_cents {
            let total = quote.total_cents;
            self.cancel_transaction(quote.transaction_id).await;
            return Err(Error::PurchaseRefused(format!(
                "the total {total} is more than the {max_total_cents} allowed; the transaction was cancelled"
            )));
        }
        let checkout = self.session.urls().checkout.clone();
        let transid = quote.transaction_id.to_string();
        let browser_info =
            r#"{"language":"en-US","javaEnabled":"false","colorDepth":24,"screenHeight":1080,"screenWidth":1920}"#;
        let form = [("transid", transid.clone()), ("CardCVV2", String::new()), ("browserInfo", browser_info.into())];
        let unconfirmed = |e: Error| Error::PurchaseUnconfirmed(e.to_string());
        let sent = self.session.post(&format!("{checkout}/checkout/finalizetransaction/"), &form, None).await;
        let mut status: StatusResponse = json(sent.map_err(unconfirmed)?).await.map_err(unconfirmed)?;
        for _ in 0..POLL_ATTEMPTS {
            match outcome(&status) {
                Some(result) => return result,
                None => {
                    tokio::time::sleep(POLL_INTERVAL).await;
                    let query = [("count", "1".to_string()), ("transid", transid.clone())];
                    match self.session.get(&format!("{checkout}/checkout/transactionstatus/"), &query).await {
                        Ok(response) => match json(response).await {
                            Ok(next) => status = next,
                            Err(e) => tracing::warn!("reading the purchase status failed: {e}"),
                        },
                        Err(e) => tracing::warn!("asking for the purchase status failed: {e}"),
                    }
                }
            }
        }
        Err(Error::PurchaseUnconfirmed("Steam was still processing it".into()))
    }
}

/// The `inittransaction` form of a wallet purchase of the account cart, with
/// every field `checkout.js` sends, the card, address and gift ones empty.
fn wallet_form(session_id: &str) -> Vec<(&'static str, String)> {
    let mut form: Vec<(&'static str, String)> = [
        ("gidShoppingCart", "-1"),
        ("gidReplayOfTransID", "-1"),
        ("bUseAccountCart", "1"),
        ("PaymentMethod", "steamaccount"),
        ("abortPendingTransactions", "0"),
        ("bHasCardInfo", "0"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_string()))
    .collect();
    let empty = [
        "CardNumber",
        "CardExpirationYear",
        "CardExpirationMonth",
        "FirstName",
        "LastName",
        "Address",
        "AddressTwo",
        "Country",
        "City",
        "State",
        "PostalCode",
        "Phone",
        "ShippingFirstName",
        "ShippingLastName",
        "ShippingAddress",
        "ShippingAddressTwo",
        "ShippingCountry",
        "ShippingCity",
        "ShippingState",
        "ShippingPostalCode",
        "ShippingPhone",
    ];
    form.extend(empty.into_iter().map(|k| (k, String::new())));
    form.push(("bIsGift", "0".into()));
    for k in
        ["GifteeAccountID", "GifteeEmail", "GifteeName", "GiftMessage", "Sentiment", "Signature", "ScheduledSendOnDate"]
    {
        form.push((k, String::new()));
    }
    for k in ["BankAccount", "BankCode", "BankIBAN", "BankBIC", "TPBankID", "BankAccountID"] {
        form.push((k, String::new()));
    }
    form.extend([
        ("bSaveBillingAddress", "0".to_string()),
        ("gidPaymentID", String::new()),
        ("bUseRemainingSteamAccount", "1".into()),
        ("bPreAuthOnly", "0".into()),
        ("sessionid", session_id.to_string()),
    ]);
    form
}

fn is_wallet_or_undecided(method: EPaymentMethod) -> bool {
    method == EPaymentMethod::None || method == EPaymentMethod::Wallet
}

/// Why the wallet can't pay `price`, if it can't.
fn wallet_refusal(price: &FinalPriceResponse) -> Option<String> {
    let Some(total) = price.total else {
        return Some("Steam sent no total".into());
    };
    if total < 0 {
        return Some(format!("Steam sent a negative total {total}"));
    }
    if price.require_cvv {
        return Some("Steam asks for a card's security code".into());
    }
    if let Some(method) = price.provider_payment_method.map(EPaymentMethod).filter(|&m| !is_wallet_or_undecided(m)) {
        return Some(format!("part of it would be paid with {method:?}"));
    }
    match price.steam_account_balance {
        Some(balance) if balance < total => Some(format!("the wallet has {balance} and the total is {total}")),
        _ => None,
    }
}

fn purchase_error(success: i32, detail: Option<i32>) -> Error {
    Error::Steam {
        result: EResult(success),
        message: detail.map(|d| format!("purchase result detail {:?}", EPurchaseResultDetail(d))),
    }
}

/// How a status ended, or `None` while Steam is still processing it (as
/// `checkout.js` reads `success` 22 and its details).
fn outcome(status: &StatusResponse) -> Option<Result<PurchaseOutcome>> {
    const PENDING: i32 = 22;
    let detail = status.purchaseresultdetail.unwrap_or_default();
    match (status.success, detail) {
        (1, _) | (PENDING, 29) => Some(Ok(PurchaseOutcome::Completed(status.purchasereceipt.clone()))),
        (PENDING, 66) => Some(Ok(PurchaseOutcome::Pending)),
        (PENDING, 86) => Some(Err(Error::PurchaseUnconfirmed(
            "Steam asks for a card authentication, which a wallet purchase never needs".into(),
        ))),
        (PENDING, _) => None,
        (success, _) => Some(Err(purchase_error(success, status.purchaseresultdetail))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(json: &str) -> FinalPriceResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_wallet_must_pay_all_of_it() {
        assert_eq!(
            wallet_refusal(&price(
                r#"{"success":1,"total":1329,"steamAccountBalance":2000,"providerpaymentmethod":0}"#
            )),
            None
        );
        assert!(wallet_refusal(&price(r#"{"success":1,"total":1329,"steamAccountBalance":1000}"#)).is_some());
        assert!(wallet_refusal(&price(r#"{"success":1,"total":1329,"providerpaymentmethod":2}"#)).is_some());
        assert!(wallet_refusal(&price(r#"{"success":1,"total":1329,"requirecvv":1}"#)).is_some());
        assert!(wallet_refusal(&price(r#"{"success":1}"#)).is_some());
    }

    #[test]
    fn no_card_or_bank_details_are_sent() {
        let form = wallet_form("abc");
        let get = |k: &str| form.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("PaymentMethod"), Some("steamaccount"));
        assert_eq!(get("bUseRemainingSteamAccount"), Some("1"));
        for field in ["CardNumber", "BankIBAN", "gidPaymentID", "Country"] {
            assert_eq!(get(field), Some(""), "{field}");
        }
        assert_eq!(get("sessionid"), Some("abc"));
    }

    #[test]
    fn statuses_as_checkout_js_reads_them() {
        let status = |json: &str| serde_json::from_str::<StatusResponse>(json).unwrap();
        assert!(matches!(outcome(&status(r#"{"success":1}"#)), Some(Ok(PurchaseOutcome::Completed(_)))));
        assert!(matches!(
            outcome(&status(r#"{"success":22,"purchaseresultdetail":29}"#)),
            Some(Ok(PurchaseOutcome::Completed(_)))
        ));
        assert!(matches!(
            outcome(&status(r#"{"success":22,"purchaseresultdetail":66}"#)),
            Some(Ok(PurchaseOutcome::Pending))
        ));
        assert!(outcome(&status(r#"{"success":22,"purchaseresultdetail":0}"#)).is_none());
        let failed = outcome(&status(r#"{"success":2,"purchaseresultdetail":7}"#)).unwrap().unwrap_err();
        assert_eq!(failed.result(), Some(EResult::Fail));
    }
}
