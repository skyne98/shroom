//! cargo run --release -p shroom-ui --example hl-bench
fn main() {
    let code = include_str!("../src/markdown.rs");
    shroom_ui::highlight::highlight("rust", "fn warm() {}");
    let start = std::time::Instant::now();
    let lines = shroom_ui::highlight::highlight("rust", code);
    let took = start.elapsed();
    println!(
        "{} lines, {} KB in {:?} ({:.1} MB/s)",
        lines.len(),
        code.len() / 1024,
        took,
        code.len() as f64 / took.as_secs_f64() / 1e6
    );
}
