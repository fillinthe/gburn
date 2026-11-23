//! Dev tool: list the EZCAD2 / LMC board's interfaces and endpoints (type, address, packet size), to see what the
//! galvo driver should open. `cargo run -p gburn-app --example usb_endpoints`
use nusb::MaybeFuture;

fn main() {
    for d in nusb::list_devices().wait().unwrap() {
        if d.vendor_id() != 0x9588 {
            continue;
        }
        println!("{:04x}:{:04x} {:?} {:?}", d.vendor_id(), d.product_id(), d.manufacturer_string(), d.product_string());
        let dev = d.open().wait().expect("open");
        for c in dev.configurations() {
            println!("configuration {}", c.configuration_value());
            for i in c.interfaces() {
                for a in i.alt_settings() {
                    println!("  interface {} alt {} class {:02x}", a.interface_number(), a.alternate_setting(), a.class());
                    for e in a.endpoints() {
                        println!("    endpoint 0x{:02x} {:?} {:?} max packet {} interval {}", e.address(), e.direction(), e.transfer_type(), e.max_packet_size(), e.interval());
                    }
                }
            }
        }
    }
}
