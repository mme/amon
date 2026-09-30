use amon_term::ShadowTerminal;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(std::env::args().nth(1).expect("raw"))?;
    let mut sh = ShadowTerminal::new(110, 40)?;
    sh.feed(&bytes);
    for line in sh.detection_text().lines() {
        if line.trim().is_empty() {
            continue;
        }
        println!("{line}");
    }
    Ok(())
}
