//! Real messages captured with NetHook (fixtures from steamkit-rs / SteamKit2).

use steamkit::handlers::apps::PicsProductInfoResult;
use steamkit::protos::steam::CMsgClientPicsProductInfoResponse;
use steamkit::{EMsg, EResult, Packet};

fn load(name: &str) -> Packet {
    let path = format!("{}/tests/data/pics/{name}", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(path).unwrap();
    Packet::parse(data.into()).unwrap()
}

fn product_info(name: &str) -> PicsProductInfoResult {
    let packet = load(name);
    assert_eq!(packet.emsg, EMsg::ClientPICSProductInfoResponse);
    assert!(packet.is_proto());
    let response: CMsgClientPicsProductInfoResponse = packet.decode().unwrap();
    let mut result = PicsProductInfoResult::default();
    result.extend(response);
    result
}

#[test]
fn pics_app_480() {
    let result = product_info("002_in_8904_k_EMsgClientPICSProductInfoResponse_app480.bin");
    let app = &result.apps[&480];
    assert!(app.change_number > 0);
    let kv = &app.key_values;
    assert_eq!(kv.name, "appinfo");
    assert_eq!(kv["appid"].as_u32(), Some(480));
    assert_eq!(kv["common"]["name"].as_str(), Some("Spacewar"));
    assert!(kv["depots"].children().len() > 1);
}

#[test]
fn pics_app_480_metadata_only() {
    let result = product_info("001_in_8904_k_EMsgClientPICSProductInfoResponse_app480_metadata.bin");
    let app = &result.apps[&480];
    assert!(app.change_number > 0);
    assert_eq!(app.sha.len(), 20);
    assert!(app.key_values.children().is_empty());
}

#[test]
fn pics_package_0() {
    let result = product_info("003_in_8904_k_EMsgClientPICSProductInfoResponse_sub0.bin");
    let package = &result.packages[&0];
    let kv = &package.key_values;
    assert_eq!(kv.name, "0");
    assert_eq!(kv["packageid"].as_u32(), Some(0));
    assert!(kv["appids"].children().len() > 10);
}

#[test]
fn enums_have_names_and_aliases() {
    assert_eq!(EMsg::ClientLogon.0, 5514);
    assert_eq!(EMsg::BaseGeneral, EMsg::GenericReply);
    assert_eq!(format!("{:?}", EMsg::ClientLogOnResponse), "EMsg::ClientLogOnResponse");
    assert_eq!(format!("{:?}", EMsg(999_999)), "EMsg(999999)");
    assert_eq!(EMsg::from_name("Multi"), Some(EMsg::Multi));
    assert_eq!(EResult::OK.0, 1);
    assert_eq!(EResult(84).name(), Some("RateLimitExceeded"));
}
