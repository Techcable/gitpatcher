#[rustversion::nightly]
fn main() {
    // TODO: Eliminate this hack
    println!("cargo:rustc-cfg=backtrace");
}

#[rustversion::not(nightly)]
fn main() {}
