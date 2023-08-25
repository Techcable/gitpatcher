pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    vergen::EmitBuilder::builder()
        // Used to give CLI version
        .git_describe(
            /* dirty */ true,
            /* tags */ true,
            /* match */ Some(r"v*"),
        )
        .emit()?;
    Ok(())
}
