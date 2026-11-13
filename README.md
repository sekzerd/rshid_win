# rshid_win

A Rust USB HID library for Windows — a Rust port of
[`gohid_win`](https://github.com/sekzerd/gohid_win).

## How it works

The crate talks to the Windows HID stack directly, exactly like `gohid_win`
(and `hidapi` internally):

- `setupapi.dll` enumerates the HID device interfaces,
- `hid.dll` queries the device attributes, string descriptors and report
  capabilities and exchanges feature reports,
- `cfgmgr32.dll` groups the top-level collections of one physical device.

No driver installation, C toolchain or extra DLL is required. The Win32
bindings come from the [`windows-sys`](https://crates.io/crates/windows-sys)
crate.

## Requirements

- Windows
- Rust 1.85 or newer

## Usage

### 1. Enumerate HID devices

```rust
let devices = rshid_win::enumerate()?;

let matched: Vec<&rshid_win::HidDevice> = devices
    .iter()
    .filter(|device| device.vendor_id == vid && device.product_id == pid)
    .collect();
```

`enumerate` groups the top-level collections of one physical device into a
single `HidDevice`. Each collection becomes a `HidDeviceEndpoint` in
`device.endpoints`.

### 2. Write a report to an endpoint

```rust
for device in &mut devices {
    if device.vendor_id != vid || device.product_id != pid {
        continue;
    }

    for endpoint in &mut device.endpoints {
        // 2.1 Input-only collections reject writes.
        if endpoint.output_length == 0 {
            continue;
        }

        // 2.2 Open the collection.
        endpoint.open()?;

        // 2.3 Write a report. The first byte is the report ID; the library
        //     zero pads the buffer to `output_length` for you.
        let written = endpoint.write_report(&[0x00, 0x55, 0x65])?;

        // 2.4 Read the reply. Blocks until the device sends one.
        if endpoint.input_length != 0 {
            let reply = endpoint.read_report()?;
            println!("{reply:02X?}");
        }

        // 2.5 Close the collection. Dropping the endpoint does this too.
        endpoint.close()?;
    }
}
```

Feature reports are available through `write_feature_report` and
`read_feature_report`.

### 3. Run the example

```text
cargo run --example write_report -- --vid 0x1234 --pid 0x4321 --data "55 65 01"
cargo run --example write_report -- --vid 0x1234 --pid 0x4321 --list
```

## API mapping

| `gohid_win`                              | `rshid_win`                                 |
| ---------------------------------------- | ------------------------------------------- |
| `HIDDevice`                              | `HidDevice`                                 |
| `HIDDevice.VendorID` / `ProductID` / ... | `HidDevice::vendor_id` / `product_id` / ... |
| `HIDDevice.HIDDeviceEndpoint`            | `HidDevice::endpoints`                      |
| `HIDDeviceEndpoint`                      | `HidDeviceEndpoint`                         |
| `Enumerate()`                            | `enumerate()`                               |
| `ep.Open()` / `ep.Close()`               | `endpoint.open()` / `endpoint.close()`      |
| `ep.WriteReport()`                       | `endpoint.write_report()`                   |
| `ep.ReadReport()`                        | `endpoint.read_report()`                    |
| `ep.WriteFeatureReport()`                | `endpoint.write_feature_report()`           |
| `ep.ReadFeatureReport(id)`               | `endpoint.read_feature_report(id)`          |
| `ErrNotOpen`                             | `Error::NotOpen`                            |

## Behaviour notes

- Reports are addressed by a report ID stored in the **first byte**. Reports
  passed to the write functions must start with that ID; shorter reports are
  zero padded to the endpoint's report length because Windows rejects buffers
  of any other size.
- `open` is a no-op when the endpoint is already open, and `close` is a no-op
  when it is not.
- `HidDeviceEndpoint` closes its handle on `Drop`, so explicit `close` calls
  are optional.
- Endpoints are `Send`, so one can be moved into a worker thread (for example
  to block in `read_report`).
- Errors are reported as `rshid_win::Error`; the underlying Win32 error is
  available through `std::error::Error::source`.

## Building and testing

```text
cargo build
cargo test
```

`enumerate` and the report functions need real hardware; the unit tests cover
the parts that can run without a device.

## License

[BSD 3-Clause](LICENSE)
