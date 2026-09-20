#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod ledger;

#[cfg(test)]
mod tests {
    include!("tests.rs");
}
