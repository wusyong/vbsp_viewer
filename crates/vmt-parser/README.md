# vmt-parser

Parser for source engine material files.

## Usage

```rust
use vmt_parser::from_str;
use std::fs::read_to_string;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw = read_to_string("material.vmt")?;
    let material = from_str(&raw)?;
    println!("texture: {}", material.base_texture());
    Ok(())
}
```

## Material support

Because this crate focuses on extracting common rendering parameters from the
materials, it only supports a fixed set of materials. If you need to parse a
material that isn't supported by this crate, you can do a more manual parsing by
using [vdf-reader](https://crates.io/crates/vdf-reader) directly.
