//! The web side against a local server standing in for Steam.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{Answer, Server, ME};
use futures_util::future::BoxFuture;
use steamwire::enums::{EMobileConfirmationType, ETradeOfferState};
use steamwire::handlers::auth::guard::{self, Secret};
use steamwire::{EResult, SteamId};
use steamwire_community::{
    Asset, CartEntry, Confirmations, Error, Inventories, NewOffer, OfferFilter, PurchaseOutcome, Store, TokenIssuer,
    TradeLink, TradeOffers, Urls, WebSession,
};

const OFFER: &str = r#"{"tradeofferid":"4127395150","accountid_other":79925588,"trade_offer_state":2,
    "items_to_receive":[{"appid":440,"contextid":"2","assetid":"5","classid":"1","instanceid":"0","amount":"1"}],
    "is_our_offer":false,"time_created":1,"time_updated":1,"expiration_time":2,"confirmation_method":0}"#;

#[tokio::test]
async fn session_cookies_and_expiry() {
    let server = Server::start(|request| match request.path.as_str() {
        "/account/" if request.cookie("steamLoginSecure").is_some() => {
            Answer::status(200, "").header("Set-Cookie", "webTradeEligibility=abc; path=/")
        }
        _ => Answer::redirect("https://store.steampowered.com/login/?redir=account"),
    })
    .await;
    let session = server.session();
    assert!(session.is_logged_in().await.unwrap());
    let first = server.last();
    assert_eq!(first.cookie("steamLoginSecure").as_deref(), Some("76561198000000001%7C%7Cheader.payload.signature"));
    assert_eq!(first.cookie("sessionid").as_deref(), Some(session.session_id()));
    assert_eq!(session.session_id().len(), 24);

    // Cookies Steam sets are sent back.
    assert!(session.is_logged_in().await.unwrap());
    assert_eq!(server.last().cookie("webTradeEligibility").as_deref(), Some("abc"));

    // A redirect to the login page means the session is over.
    let expired = WebSession::builder(ME).urls(Urls::all(&format!("{}/gone", server.url))).access_token("x").unwrap();
    assert!(!expired.is_logged_in().await.unwrap());
}

struct CountingIssuer(AtomicUsize);

impl TokenIssuer for CountingIssuer {
    fn issue(&self) -> BoxFuture<'_, steamwire_community::Result<String>> {
        Box::pin(async move {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            // A token that is already due for renewal: it lasts 10 s.
            use base64::Engine as _;
            let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"iat":0,"exp":10}"#);
            Ok(format!("h.{claims}.{n}"))
        })
    }
}

#[tokio::test]
async fn tokens_are_renewed_through_the_issuer() {
    let issuer = Arc::new(CountingIssuer(AtomicUsize::new(0)));
    let session = WebSession::builder(ME).issuer(issuer.clone()).await.unwrap();
    assert!(session.access_token().await.unwrap().expose_secret().ends_with(".0"));
    // Not again right away...
    assert!(session.access_token().await.unwrap().expose_secret().ends_with(".0"));
    assert_eq!(issuer.0.load(Ordering::SeqCst), 1);
    assert_eq!(format!("{:?}", session.access_token().await.unwrap()), "AccessToken(..)");
}

#[tokio::test]
async fn confirmations_are_listed_and_answered() {
    let server = Server::start(|request| match request.path.as_str() {
        "/mobileconf/getlist" => Answer::json(
            r#"{"success":true,"conf":[{"type":2,"type_name":"Trade Offer","id":"13000000001","creator_id":"4127395150",
                "nonce":"18064583892738866189","creation_time":1700000000,"headline":"someone","summary":["You will give 1 item"]},
               {"type":3,"type_name":"Market Listing","id":"13000000002","creator_id":"5","nonce":"1","creation_time":1}]}"#,
        ),
        "/mobileconf/multiajaxop" => Answer::json(r#"{"success":true}"#),
        _ => Answer::status(404, ""),
    })
    .await;
    let secret = Secret::new(b"identity secret".to_vec());
    let confirmations = Confirmations::new(server.session(), secret.clone());
    let list = confirmations.list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].kind, EMobileConfirmationType::Trade);
    assert_eq!(list[0].trade_offer_id(), Some(4127395150));
    assert_eq!(list[0].key, 18064583892738866189);
    assert_eq!(list[1].trade_offer_id(), None);

    let request = server.last();
    assert_eq!(request.query("a"), Some("76561198000000001"));
    assert_eq!(request.query("p"), Some(guard::device_id("76561198000000001").as_str()));
    assert_eq!(request.query("m"), Some("react"));
    assert_eq!(request.query("tag"), Some("conf"));
    let time: u64 = request.query("t").unwrap().parse().unwrap();
    assert_eq!(request.query("k"), Some(guard::generate_confirmation_key(&secret, time, "conf").as_str()));

    assert!(confirmations.accept_trade_offer(4127395150).await.unwrap());
    let answer = server.last();
    assert_eq!(answer.method, "POST");
    assert_eq!(answer.field("op"), Some("allow"));
    assert_eq!(answer.form["cid[]"], ["13000000001"]);
    assert_eq!(answer.form["ck[]"], ["18064583892738866189"]);

    confirmations.cancel(&list[1..]).await.unwrap();
    assert_eq!(server.last().field("op"), Some("cancel"));
}

#[tokio::test]
async fn confirmations_need_a_session() {
    let server = Server::start(|_| Answer::json(r#"{"success":false,"needauth":true}"#)).await;
    let confirmations = Confirmations::new(server.session(), Secret::new(vec![1]));
    assert!(matches!(confirmations.list().await, Err(Error::SessionExpired)));
}

#[tokio::test]
async fn trade_offers() {
    let server = Server::start(|request| match request.path.as_str() {
        "/IEconService/GetTradeOffers/v1/" => match request.query("cursor") {
            None => Answer::json(&format!(r#"{{"response":{{"trade_offers_received":[{OFFER}],"next_cursor":7}}}}"#)),
            // The second page repeats the offer, which is only listed once.
            Some(_) => Answer::json(&format!(r#"{{"response":{{"trade_offers_received":[{OFFER}],"next_cursor":0}}}}"#)),
        },
        "/IEconService/GetTradeOffer/v1/" => Answer::json(&format!(r#"{{"response":{{"offer":{OFFER}}}}}"#)),
        "/IEconService/GetTradeHoldDurations/v1/" => Answer::json(
            r#"{"response":{"my_escrow":{"escrow_end_duration_seconds":0},"their_escrow":{"escrow_end_duration_seconds":"86400"}}}"#,
        ),
        "/tradeoffer/new/send" => {
            Answer::json(r#"{"tradeofferid":"9438483375","needs_mobile_confirmation":true,"needs_email_confirmation":false}"#)
        }
        "/tradeoffer/4127395150/accept" => Answer::json(r#"{"tradeid":"1234"}"#),
        "/tradeoffer/4127395150/decline" => Answer::json(r#"{"tradeofferid":"4127395150"}"#),
        "/tradeoffer/1/cancel" => Answer::status(
            500,
            r#"{"strError":"There was an error cancelling this trade offer. (26)"}"#,
        ),
        _ => Answer::status(404, ""),
    })
    .await;
    let offers = TradeOffers::new(server.session());

    let list = offers.list(OfferFilter::default()).await.unwrap();
    assert_eq!(list.received.len(), 1);
    let offer = &list.received[0];
    assert_eq!(offer.state, ETradeOfferState::Active);
    assert_eq!(offer.partner, SteamId::from_u64(76561198040191316));
    let pages = server.requests();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].query("access_token"), Some("header.payload.signature"));
    assert_eq!(pages[0].query("active_only"), Some("1"));
    assert_eq!(pages[1].query("cursor"), Some("7"));

    assert_eq!(offers.get(4127395150).await.unwrap(), *offer);

    let link = TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=79925588&token=Ob27qXzn").unwrap();
    let hold = offers.hold_durations(&link).await.unwrap();
    assert_eq!((hold.mine.as_secs(), hold.theirs.as_secs()), (0, 86400));
    assert_eq!(server.last().query("trade_offer_access_token"), Some("Ob27qXzn"));

    let sent = offers.send(&NewOffer::to(link).give(Asset::new(730, 2, 1)).message("hi")).await.unwrap();
    assert_eq!(sent.offer_id, Some(9438483375));
    assert!(sent.needs_mobile_confirmation);
    let request = server.last();
    assert_eq!(request.field("partner"), Some("76561198040191316"));
    assert_eq!(request.field("tradeoffermessage"), Some("hi"));
    assert_eq!(
        request.headers.get("referer").map(String::as_str),
        Some(format!("{}/tradeoffer/new", server.url).as_str())
    );

    let accepted = offers.accept(offer).await.unwrap();
    assert_eq!(accepted.trade_id, Some(1234));
    let request = server.last();
    assert_eq!(request.field("partner"), Some("76561198040191316"));
    assert_eq!(request.field("tradeofferid"), Some("4127395150"));
    assert_eq!(request.field("sessionid"), request.cookie("sessionid").as_deref());

    offers.decline(4127395150).await.unwrap();
    let error = offers.cancel(1).await.unwrap_err();
    assert_eq!(error.result(), Some(EResult::Revoked));
}

#[tokio::test]
async fn inventories_are_read_page_by_page() {
    let server = Server::start(|request| match request.query("start_assetid") {
        None => Answer::json(
            r#"{"success":1,"assets":[{"appid":730,"contextid":"2","assetid":"1","classid":"10","instanceid":"0","amount":"1"}],
                "descriptions":[{"appid":730,"classid":"10","instanceid":"0","name":"AK-47","market_hash_name":"AK-47 | Redline","type":"Rifle","tradable":1,"marketable":1}],
                "more_items":1,"last_assetid":"1","total_inventory_count":2}"#,
        ),
        Some("1") => Answer::json(
            r#"{"success":1,"assets":[{"appid":730,"contextid":"2","assetid":"2","classid":"11","instanceid":"0","amount":"1"}],
                "descriptions":[{"appid":730,"classid":"11","instanceid":"0","name":"Case","tradable":0}],"total_inventory_count":2}"#,
        ),
        _ => Answer::status(404, ""),
    })
    .await;
    let items = Inventories::new(server.session()).own(730, 2).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].asset, Asset::new(730, 2, 1));
    let description = items[0].description.as_ref().unwrap();
    assert_eq!(description.market_hash_name, "AK-47 | Redline");
    assert!(description.tradable && description.marketable);
    assert!(!items[1].description.as_ref().unwrap().tradable);
    let request = server.last();
    assert_eq!(request.path, "/inventory/76561198000000001/730/2");
    assert_eq!(request.query("count"), Some("1000"));

    let private = Server::start(|_| Answer::status(403, "null")).await;
    let error = Inventories::new(private.session()).of(SteamId::from_account_id(5), 730, 2).await.unwrap_err();
    assert!(matches!(error, Error::HttpStatus(403)));
}

fn checkout(final_price: &'static str, status: &'static str) -> impl Fn(&common::Request) -> Answer + Send + Sync {
    move |request| match request.path.as_str() {
        "/checkout/inittransaction/" => Answer::json(r#"{"success":1,"transid":"5555","paymentmethod":0}"#),
        "/checkout/getfinalprice/" => Answer::json(final_price),
        "/checkout/finalizetransaction/" => Answer::json(status),
        "/checkout/canceltransaction/" => Answer::json(r#"{"success":1}"#),
        _ => Answer::status(404, ""),
    }
}

#[tokio::test]
async fn wallet_purchase() {
    let server = Server::start(checkout(
        r#"{"success":1,"total":1329,"steamAccountBalance":2000,"providerpaymentmethod":0,"formattedTotal":"13,29€"}"#,
        r#"{"success":1,"purchasereceipt":{"transactionid":"5555","formattedTotal":"13,29€","paymentmethod":128}}"#,
    ))
    .await;
    let store = Store::new(server.session());
    let quote = store.quote().await.unwrap();
    assert_eq!((quote.transaction_id, quote.total_cents), (5555, 1329));
    let init = &server.requests()[0];
    assert_eq!(init.field("PaymentMethod"), Some("steamaccount"));
    assert_eq!(init.field("bUseAccountCart"), Some("1"));
    assert_eq!(init.field("CardNumber"), Some(""));
    assert_eq!(server.last().query("transid"), Some("5555"));

    match store.finalize(quote, 1500).await.unwrap() {
        PurchaseOutcome::Completed(Some(receipt)) => assert_eq!(receipt.transaction_id, Some(5555)),
        other => panic!("{other:?}"),
    }
    let finalize = server.last();
    assert_eq!(finalize.path, "/checkout/finalizetransaction/");
    assert_eq!(finalize.field("CardCVV2"), Some(""));
}

#[tokio::test]
async fn purchases_over_the_limit_are_cancelled() {
    let server =
        Server::start(checkout(r#"{"success":1,"total":1329,"steamAccountBalance":2000}"#, r#"{"success":1}"#)).await;
    let store = Store::new(server.session());
    let quote = store.quote().await.unwrap();
    assert!(matches!(store.finalize(quote, 1000).await, Err(Error::PurchaseRefused(_))));
    let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
    assert!(!paths.iter().any(|p| p.contains("finalize")), "{paths:?}");
    assert_eq!(paths.last().map(String::as_str), Some("/checkout/canceltransaction/"));
}

#[tokio::test]
async fn quotes_the_wallet_cant_pay_are_refused() {
    for price in [
        r#"{"success":1,"total":1329,"steamAccountBalance":1000}"#,
        r#"{"success":1,"total":1329,"providerpaymentmethod":4}"#,
    ] {
        let server = Server::start(checkout(price, r#"{"success":1}"#)).await;
        let error = Store::new(server.session()).quote().await.unwrap_err();
        assert!(matches!(error, Error::PurchaseRefused(_)), "{error}");
        assert_eq!(server.last().path, "/checkout/canceltransaction/");
    }
    // Steam choosing a card at the start is refused too.
    let server = Server::start(|request| match request.path.as_str() {
        "/checkout/inittransaction/" => Answer::json(r#"{"success":1,"transid":"5555","paymentmethod":2}"#),
        _ => Answer::json(r#"{"success":1}"#),
    })
    .await;
    assert!(matches!(Store::new(server.session()).quote().await, Err(Error::PurchaseRefused(_))));
    assert_eq!(server.last().path, "/checkout/canceltransaction/");
}

#[tokio::test]
async fn failed_purchases_report_the_result() {
    let server = Server::start(|request| match request.path.as_str() {
        "/checkout/inittransaction/" => Answer::json(r#"{"success":2,"transid":"-1","purchaseresultdetail":7}"#),
        _ => Answer::status(404, ""),
    })
    .await;
    let error = Store::new(server.session()).quote().await.unwrap_err();
    assert_eq!(error.result(), Some(EResult::Fail));
}

#[tokio::test]
async fn the_cart() {
    use prost::Message;

    // AccountCartContents with one package line and a subtotal.
    #[derive(Clone, PartialEq, prost::Message)]
    struct Amount {
        #[prost(int64, optional, tag = "1")]
        cents: Option<i64>,
        #[prost(int32, optional, tag = "2")]
        currency: Option<i32>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct Line {
        #[prost(uint64, optional, tag = "1")]
        id: Option<u64>,
        #[prost(uint32, optional, tag = "3")]
        packageid: Option<u32>,
        #[prost(bool, optional, tag = "8")]
        valid: Option<bool>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct Contents {
        #[prost(message, repeated, tag = "1")]
        lines: Vec<Line>,
        #[prost(message, optional, tag = "2")]
        subtotal: Option<Amount>,
        #[prost(bool, optional, tag = "3")]
        valid: Option<bool>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct AddResponse {
        #[prost(message, optional, tag = "2")]
        cart: Option<Contents>,
    }
    let contents = Contents {
        lines: vec![Line { id: Some(9), packageid: Some(54029), valid: Some(true) }],
        subtotal: Some(Amount { cents: Some(1329), currency: Some(3) }),
        valid: Some(true),
    };
    let body = AddResponse { cart: Some(contents) }.encode_to_vec();
    let server = Server::start(move |request| match request.path.as_str() {
        "/IAccountCartService/AddItemsToCart/v1/" => {
            Answer { status: 200, headers: vec![("x-eresult".into(), "1".into())], body: body.clone() }
        }
        _ => Answer::status(404, "").header("x-eresult", "2"),
    })
    .await;
    let store = Store::new(server.session());
    let cart = store.add_to_cart(&[CartEntry::Package(54029)]).await.unwrap();
    assert_eq!(cart.items[0].package_id, Some(54029));
    assert_eq!(cart.items[0].line_item_id, 9);
    assert_eq!(cart.subtotal.as_ref().map(|a| a.cents), Some(1329));
    let request = server.last();
    assert_eq!(request.method, "POST");
    assert_eq!(request.field("format"), Some("protobuf_raw"));
    assert!(request.field("input_protobuf_encoded").is_some());

    let error = store.cart().await.unwrap_err();
    assert_eq!(error.result(), Some(EResult::Fail));
}
