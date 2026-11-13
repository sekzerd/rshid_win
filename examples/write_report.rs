//! Lists HID devices and sends one report to a matching one.
//!
//! ```text
//! cargo run --example write_report -- --vid 0x1234 --pid 0x4321 --data "55 65 01"
//! ```

use std::error::Error;
use std::process::ExitCode;

const USAGE: &str = "\
Usage: write_report [OPTIONS]

Options:
    --vid HEX   vendor id (16 bit, default 0x1234)
    --pid HEX   product id (16 bit, default 0x4321)
    --data HEX  report bytes starting with the report id (default \"55 65 01\")
    --list      only list matching devices
    -h, --help  print this help
";

const DEFAULT_VENDOR_ID: u16 = 0x1234;
const DEFAULT_PRODUCT_ID: u16 = 0x4321;
const DEFAULT_DATA: &str = "55 65 01";

struct Args {
    vendor_id: u16,
    product_id: u16,
    data: String,
    list: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Self {
            vendor_id: DEFAULT_VENDOR_ID,
            product_id: DEFAULT_PRODUCT_ID,
            data: DEFAULT_DATA.to_owned(),
            list: false,
        };

        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--vid" | "-vid" => {
                    let value = next_value(&mut arguments, &argument)?;
                    args.vendor_id = parse_u16(&value, &argument)?;
                }
                "--pid" | "-pid" => {
                    let value = next_value(&mut arguments, &argument)?;
                    args.product_id = parse_u16(&value, &argument)?;
                }
                "--data" | "-data" => {
                    args.data = next_value(&mut arguments, &argument)?;
                }
                "--list" | "-list" => args.list = true,
                "-h" | "--help" => {
                    print!("{USAGE}");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
            }
        }

        Ok(args)
    }
}

fn next_value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u16(value: &str, flag: &str) -> Result<u16, String> {
    let value = value.trim();
    let parsed = match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse::<u32>(),
    };
    let parsed = parsed.map_err(|_| format!("{flag} expects a number, got {value:?}"))?;
    u16::try_from(parsed).map_err(|_| format!("{flag} must fit in 16 bits, got {value:?}"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = Args::parse()?;
    let payload = parse_hex_bytes(&args.data)?;

    let mut devices = rshid_win::enumerate()?;
    let matched: Vec<usize> = devices
        .iter()
        .enumerate()
        .filter(|(_, device)| {
            device.vendor_id == args.vendor_id && device.product_id == args.product_id
        })
        .map(|(position, _)| position)
        .collect();

    if matched.is_empty() {
        return Err(format!(
            "no HID device {:04X}:{:04X} found ({} HID devices scanned)",
            args.vendor_id,
            args.product_id,
            devices.len()
        )
        .into());
    }

    for &position in &matched {
        print_device(&devices[position]);
    }
    if args.list {
        return Ok(());
    }

    // Input-only collections reject writes, so try every endpoint that
    // declares an output report until one accepts it.
    let mut skipped = Vec::new();
    for &position in &matched {
        let device = &mut devices[position];
        for endpoint in &mut device.endpoints {
            if endpoint.output_length == 0 {
                skipped.push(format!("{}: no output report", endpoint.path));
                continue;
            }

            if let Err(error) = endpoint.open() {
                skipped.push(format!("{}: {error}", endpoint.path));
                continue;
            }

            let written = match endpoint.write_report(&payload) {
                Ok(written) => written,
                Err(error) => {
                    skipped.push(format!("{}: {error}", endpoint.path));
                    let _ = endpoint.close();
                    continue;
                }
            };

            println!("data         : {}", hex(&payload));
            println!(
                "report       : {} bytes, written {}, report ID {:#04x}",
                endpoint.output_length, written, payload[0]
            );

            if endpoint.input_length != 0 {
                match endpoint.read_report() {
                    Ok(data) => println!("read data    : {}", hex(&data)),
                    Err(error) => {
                        skipped.push(format!("{}: {error}", endpoint.path));
                        let _ = endpoint.close();
                        continue;
                    }
                }
            }

            let _ = endpoint.close();
            return Ok(());
        }
    }

    for message in &skipped {
        eprintln!("skip         : {message}");
    }
    Err("no endpoint accepted the report".into())
}

fn print_device(device: &rshid_win::HidDevice) {
    println!(
        "{:04X}:{:04X} version {:#06x} release {:#06x} interface {}",
        device.vendor_id,
        device.product_id,
        device.version_number,
        device.release_number,
        device.interface_number
    );
    println!("  product      : {:?}", device.product);
    println!("  manufacturer : {:?}", device.manufacturer);
    println!("  serial       : {:?}", device.serial_number);
    for endpoint in &device.endpoints {
        println!("  endpoint     : {}", endpoint.path);
        println!(
            "                 usage page {:#06x} usage {:#06x}, in/out/feature = {}/{}/{}",
            endpoint.usage_page,
            endpoint.usage,
            endpoint.input_length,
            endpoint.output_length,
            endpoint.feature_length
        );
    }
}

/// Accepts `"55 65"`, `"55,65"`, `"0x55 0x65"` or `"5565"`.
fn parse_hex_bytes(input: &str) -> Result<Vec<u8>, String> {
    let cleaned = input
        .replace("0x", " ")
        .replace("0X", " ")
        .replace([',', '\t', '\n'], " ");
    let fields: Vec<&str> = cleaned.split_whitespace().collect();
    if fields.is_empty() {
        return Err(format!("no bytes in {input:?}"));
    }

    // Allow a contiguous hex string such as "5565".
    if fields.len() == 1 && fields[0].len() > 2 {
        let value = fields[0];
        if value.len() % 2 != 0 {
            return Err(format!("odd number of hex digits in {value:?}"));
        }
        return (0..value.len())
            .step_by(2)
            .map(|index| parse_hex_byte(&value[index..index + 2]))
            .collect();
    }

    fields.iter().map(|field| parse_hex_byte(field)).collect()
}

fn parse_hex_byte(input: &str) -> Result<u8, String> {
    if input.is_empty() || input.len() > 2 {
        return Err(format!("{input:?} is not a hex byte"));
    }
    u8::from_str_radix(input, 16).map_err(|_| format!("{input:?} is not a hex byte"))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_bytes_in_several_notations() {
        let expected = vec![0x55, 0x65, 0x01];
        for input in ["55 65 01", "55,65,01", "0x55 0x65 0x01", "556501"] {
            assert_eq!(parse_hex_bytes(input).unwrap(), expected, "input {input:?}");
        }
    }

    #[test]
    fn rejects_invalid_hex_bytes() {
        assert!(parse_hex_bytes("").is_err());
        assert!(parse_hex_bytes("555").is_err());
        assert!(parse_hex_bytes("zz").is_err());
    }

    #[test]
    fn parses_u16_values() {
        assert_eq!(parse_u16("0x1234", "--vid").unwrap(), 0x1234);
        assert_eq!(parse_u16("4660", "--vid").unwrap(), 0x1234);
        assert!(parse_u16("0x10000", "--vid").is_err());
        assert!(parse_u16("nope", "--vid").is_err());
    }
}
