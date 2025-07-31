use alloc::string::String;
// TODO: This module shouldn't really exist, but it's useful for the moment. Replace with an existing CSV crate.

pub trait ToCsv {
    fn to_csv(&self) -> String;
}
