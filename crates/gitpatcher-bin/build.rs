pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    vergen_git2::Emitter::new()
        .add_instructions(
            &vergen_git2::Git2Builder::default()
                .describe(
                    /* tags */ true,
                    /* dirty */ true,
                    /* match */ Some(r"v*"),
                )
                .build()?,
        )?
        .emit()?;
    Ok(())
}
