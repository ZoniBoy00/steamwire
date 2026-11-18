//! Shows what the machine ID sent at logon is made of on this system, and
//! the OS type. It prints only which parts were found and their sizes, not
//! the IDs themselves. Fails if the system gave none of them.
//!
//! ```sh
//! cargo run -p steamwire --example machine_info
//! ```

use steamwire::util::hardware::{machine_id, MachineInfoProvider, SystemMachineInfoProvider};
use steamwire::util::os::os_type;

fn main() {
    let provider = SystemMachineInfoProvider;
    let parts = [
        ("machine GUID", provider.machine_guid()),
        ("MAC addresses", provider.mac_address()),
        ("disk ID", provider.disk_id()),
    ];
    for (name, part) in &parts {
        match part {
            Some(bytes) => println!("{name}: found ({} bytes)", bytes.len()),
            None => println!("{name}: not found, a placeholder is used"),
        }
    }
    println!("machine ID: {} bytes", machine_id(&provider).len());
    println!("OS type: {:?}", os_type());

    if parts.iter().all(|(_, part)| part.is_none()) {
        eprintln!("none of the machine ID's parts were found");
        std::process::exit(1);
    }
}
