//! Set the clipboard to the first argument and hold it until interrupted.

use lamco_data_control::{Content, DataControl};

fn main() -> lamco_data_control::Result<()> {
    let text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "hello".to_owned());
    let clipboard = DataControl::connect()?;
    clipboard.set_selection(
        Content::new()
            .data("text/plain;charset=utf-8", text.clone())
            .data("text/plain", text),
    )?;
    println!("clipboard set; press Ctrl-C to release it");
    std::thread::park();
    Ok(())
}
