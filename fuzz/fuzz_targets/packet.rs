#![no_main]

use libfuzzer_sys::fuzz_target;
use steamwire::messages::gc::GcPacket;
use steamwire::messages::{generated::describe_body, split_multi, Packet};
use steamwire::net::udp::UdpPacket;

fuzz_target!(|data: &[u8]| {
    if let Ok(packet) = Packet::parse(data.to_vec().into()) {
        let _ = split_multi(&packet);
        let _ = packet.encode();
        let _ = describe_body(packet.emsg, &packet.body);
    }
    let _ = GcPacket::parse(0x8000_0000 | 4004, data.to_vec().into());
    let _ = GcPacket::parse(4004, data.to_vec().into());
    let _ = UdpPacket::parse(data);
});
