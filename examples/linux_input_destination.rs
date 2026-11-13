//! Public-API driver for scripts/repro-linux-desktop.sh (isolated X11 only).
#![allow(clippy::print_stdout)]

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use computer_use::{Computer, Delivery};
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: linux_input_destination <target-pid> <background|global> <marker>".into());
    }
    if std::env::var("CU_ISOLATED_DESKTOP").as_deref() != Ok("1") {
        return Err("run through scripts/repro-linux-desktop.sh, never on your live desktop".into());
    }
    let mode = match args[1].as_str() {
        "background" => Delivery::Background,
        "global" => Delivery::Global,
        _ => return Err("delivery must be background or global".into()),
    };
    let query = format!("pid:{}", args[0].parse::<u32>()?);
    let mut computer = Computer::new()?.delivery(mode);
    let app = computer.resolve_app(&query)?;
    println!("resolved target: {} (pid {})", app.name, app.pid.unwrap_or_default());
    // No capture or mock backend: this is the native public API.
    let result = computer.type_text(&query, &args[2]);
    println!("delivery: {mode:?}; type_text: {result:?}");
    if result.is_ok() {
        println!("RESULT=OK");
    } else if matches!(result, Err(computer_use::Error::Unsupported(_))) {
        println!("RESULT=UNSUPPORTED");
    } else {
        return Err(format!("unexpected API failure: {result:?}").into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("this reproduction requires Linux");
}
