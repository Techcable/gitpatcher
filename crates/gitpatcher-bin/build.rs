pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    vergen_gitcl::Emitter::new()
        .add_instructions(
            &vergen_gitcl::GitclBuilder::default()
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
