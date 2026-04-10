use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

mod convert;
mod parser;
mod render;
mod tokenize;

#[cfg(test)]
mod tests;

use convert::ConvertedConfig;
use parser::{ParsedNginxConfig, ParserState};
use tokenize::tokenize;

#[derive(Debug)]
pub struct MigrationOutput {
    pub ron: String,
    pub warnings: Vec<String>,
}

pub fn migrate_file(input_path: &Path) -> Result<MigrationOutput> {
    let source = fs::read_to_string(input_path)
        .with_context(|| format!("failed to read nginx config {}", input_path.display()))?;
    migrate_source(&source, &input_path.display().to_string())
}

fn migrate_source(source: &str, source_label: &str) -> Result<MigrationOutput> {
    let tokens = tokenize(source)?;
    let statements = ParserState::new(tokens).parse()?;
    let parsed = ParsedNginxConfig::from_statements(statements)?;
    let converted = ConvertedConfig::from_parsed(parsed)?;
    Ok(MigrationOutput { ron: converted.render(source_label), warnings: converted.warnings })
}
