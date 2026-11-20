//! Opens the ASIO driver AudiForm 2 would pick, plays silence for two
//! seconds and reports what opened: `cargo run --example asio_probe
//! --features asio -p af27-engine`.
fn main() {
    let drivers = af27_engine::devices::asio_drivers();
    println!("ASIO built: {}", af27_engine::ASIO_BUILT);
    println!("drivers: {drivers:?}");
    let choice = af27_engine::DeviceChoice {
        host: af27_engine::HostKind::Asio,
        ..Default::default()
    };
    match af27_engine::Engine::open(&choice) {
        Ok(e) => {
            println!(
                "opened: {} @ {} Hz, buffer {:?}",
                e.device, e.rate, e.buffer
            );
            std::thread::sleep(std::time::Duration::from_secs(2));
            println!("lost after 2 s: {}", e.is_lost());
        }
        Err(err) => println!("open failed: {err}"),
    }
}
