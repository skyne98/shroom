//! Dump markdown blocks: printf '## t\n' | cargo run -q -p shroom-ui --example md

use std::io::Read;

fn main() {
    let mut source = String::new();
    std::io::stdin().read_to_string(&mut source).unwrap();
    for block in shroom_ui::markdown::markdown(&source) {
        let spans: Vec<String> = block
            .spans
            .iter()
            .map(|span| format!("{:?}:{:?}", span.ink, span.text))
            .collect();
        println!("indent={} {}", block.indent, spans.join(" "));
    }
}
