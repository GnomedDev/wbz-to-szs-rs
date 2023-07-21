# WBZ-to-SZS-rs

A POC Rust port of the WBZ -> SZS parsing functionality present in [Wiimm's SZS Tools](https://github.com/Wiimm/wiimms-szs-tools).

This is meant to stand as an implementation easier to understand than the `wszst` implementation due to the largely reduced scope,
and has been built to teach me how this file format works for implementation in [MKW-SP](https://github.com/mkw-sp/mkw-sp).

Due to this, it is entirely acceptable for this program to summon demons when run, but for the one (1) test file I used it worked
bit perfect to `wszst decompress --u8`.

## Binary Usage

`wbz-szs-rs file.wbz` - Outputs `file.u8` which can be repackaged into an `SZS` or other Track container.

## Library Usage
See `cargo doc`.
