//! Dump how a stored body parses.
//!   jq -r .body ~/.local/share/shroom/1.json | cargo run -q -p shroom-ui --example transcript

use std::io::Read;

fn main() {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .expect("read stdin");
    let lines: Vec<String> = raw.lines().map(str::to_string).collect();
    for (index, line) in shroom_ui::transcript(&lines).iter().enumerate() {
        println!("{index:2} {:?} {:?}", line.role, line.text);
    }
}
