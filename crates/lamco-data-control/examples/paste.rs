//! Print the clipboard as text.

use lamco_data_control::DataControl;

fn main() -> lamco_data_control::Result<()> {
    let clipboard = DataControl::connect()?;
    println!("protocol: {}", clipboard.protocol());
    println!("offered: {:?}", clipboard.selection_mime_types());
    match clipboard.read("text/plain;charset=utf-8")? {
        Some(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
        None => println!("no text on the clipboard"),
    }
    Ok(())
}
