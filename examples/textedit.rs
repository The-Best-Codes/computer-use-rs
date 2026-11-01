//! End-to-end demo on macOS `TextEdit`: create a document, type into it, read the text back,
//! then select a word.
//!
//! ```sh
//! cargo run --example textedit
//! ```

#![allow(clippy::print_stdout)]

#[cfg(target_os = "macos")]
fn main() -> computer_use::Result<()> {
    use std::time::Duration;

    use computer_use::{
        CaptureOptions, ClickMethod, ClickOptions, ClickTarget, Computer, ElementQuery, Error, TextSelection,
    };

    const APP: &str = "TextEdit";
    let mut computer = Computer::new()?.settle_delay(Duration::from_millis(300));
    let tree_only = CaptureOptions::default().include_screenshot(false);

    let state = computer.get_app_state(APP, &tree_only)?;
    let new_doc = state.find(&ElementQuery::text("New Document").with_role("button").exact(true)).next();
    if let Some(button) = new_doc.map(|e| e.index) {
        println!(
            "click New Document: {:?}",
            computer.click(APP, ClickTarget::Element(button), ClickOptions::default())?
        );
    }

    let state = computer.get_app_state(APP, &tree_only)?;
    let text_area = state
        .find(&ElementQuery::role("AXTextArea"))
        .next()
        .map(|e| e.index)
        .ok_or_else(|| Error::StateUnavailable("no text area".into()))?;
    println!(
        "click text area {text_area}: {:?}",
        computer.click(APP, ClickTarget::Element(text_area), ClickOptions::default())?
    );
    computer.set_value(APP, text_area, "")?;
    computer.type_text(APP, "Hello from computer-use-rs!\nSecond line.")?;

    let state = computer.get_app_state(APP, &tree_only)?;
    let value = state.find(&ElementQuery::role("AXTextArea")).next().and_then(|e| e.value.clone());
    println!("text area now contains: {value:?}");

    computer.select_text(APP, text_area, &TextSelection::new("computer-use-rs"))?;
    let state = computer.get_app_state(APP, &tree_only)?;
    println!("selected text: {:?}", state.selected_text);

    computer.press_key(APP, "BackSpace")?;
    let state = computer.get_app_state(APP, &tree_only)?;
    let value = state.find(&ElementQuery::role("AXTextArea")).next().and_then(|e| e.value.clone());
    println!("after BackSpace: {value:?}");

    computer.press_key(APP, "super+a")?;
    let state = computer.get_app_state(APP, &tree_only)?;
    println!("after super+a, selected: {:?}", state.selected_text);

    let focused = ClickOptions::default().method(ClickMethod::FocusedBackground);
    println!("focused click: {:?}", computer.click(APP, ClickTarget::Element(text_area), focused)?);
    computer.type_text(APP, " The end.")?;
    let state = computer.get_app_state(APP, &tree_only)?;
    let value = state.find(&ElementQuery::role("AXTextArea")).next().and_then(|e| e.value.clone());
    println!("final text: {value:?}");

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
