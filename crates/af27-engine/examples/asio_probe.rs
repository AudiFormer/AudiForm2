//! Lists the ASIO drivers and opens one for a moment (by hand):
//! `cargo run --release -p af27-engine --features asio --example asio_probe`.
fn main() {
    println!("ASIO built: {}", af27_engine::ASIO_BUILT);
    let l = af27_engine::devices::list(af27_engine::HostKind::Asio, None, None);
    println!("drivers: {:?}", l.asio_drivers);
    println!("using: {:?}", l.asio_driver);
    println!("outputs: {:?}", l.outputs.iter().map(|d| &d.name).collect::<Vec<_>>());
    println!("inputs: {:?}", l.inputs.iter().map(|d| &d.name).collect::<Vec<_>>());
    println!("rates {:?}, now {:?}, buffers {:?}, error {:?}", l.rates, l.rate, l.buffer_range, l.error);
}
