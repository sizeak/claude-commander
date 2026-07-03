//! Pre-load config-file migrations.
//!
//! These operate on raw TOML before deserialisation so old config shapes can be
//! normalised once on disk while the runtime `Config` model stays simple.

use std::path::Path;
use std::str::FromStr;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};
use tracing::warn;

use crate::error::{ConfigError, Error, Result};

type Migration = fn(&mut DocumentMut) -> Result<bool>;

const MIGRATIONS: &[Migration] = &[migrate_default_program_to_programs];

pub(crate) fn migrate_config_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let src = std::fs::read_to_string(path).map_err(|e| {
        Error::Config(ConfigError::LoadFailed(format!(
            "Failed to read config file: {}",
            e
        )))
    })?;
    let mut doc = DocumentMut::from_str(&src)
        .map_err(|e| Error::Config(ConfigError::LoadFailed(e.to_string())))?;

    let mut changed = false;
    for migration in MIGRATIONS {
        changed |= migration(&mut doc)?;
    }

    if changed {
        let migrated = doc.to_string();
        if let Err(e) = std::fs::write(path, &migrated) {
            warn!(
                path = %path.display(),
                error = %e,
                "failed to persist config migration; continuing with migrated config in memory"
            );
        }

        Ok(Some(migrated))
    } else {
        Ok(Some(src))
    }
}

fn migrate_default_program_to_programs(doc: &mut DocumentMut) -> Result<bool> {
    let Some(default_program) = doc
        .get("default_program")
        .and_then(|item| item.as_str())
        .map(str::to_string)
    else {
        return Ok(false);
    };

    doc.remove("default_program");

    let programs = ensure_programs_array(doc)?;
    let positions = program_positions(programs);
    let existing: Vec<Table> = programs.iter().cloned().collect();
    let matching_index = existing.iter().position(|program| {
        program
            .get("command")
            .and_then(|item| item.as_str())
            .is_some_and(|command| command == default_program)
    });
    if let Some(index) = matching_index {
        if index > 0 {
            let mut reordered = existing;
            let program = reordered.remove(index);
            reordered.insert(0, program);
            set_programs(doc, reordered, positions);
        }
    } else {
        let mut reordered = existing;
        reordered.insert(0, program_table(&default_program));
        set_programs(doc, reordered, positions);
    }

    Ok(true)
}

fn ensure_programs_array(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    if !doc.contains_key("programs") {
        doc["programs"] = Item::ArrayOfTables(ArrayOfTables::new());
    }

    if doc["programs"].as_array_of_tables().is_none()
        && let Some(programs) = inline_programs_array_to_array_of_tables(doc["programs"].as_value())
    {
        doc.remove("programs");
        doc["programs"] = Item::ArrayOfTables(programs);
    }

    doc["programs"].as_array_of_tables_mut().ok_or_else(|| {
        Error::Config(ConfigError::LoadFailed(
            "`programs` must be an array of tables".to_string(),
        ))
    })
}

fn inline_programs_array_to_array_of_tables(value: Option<&Value>) -> Option<ArrayOfTables> {
    let array = value?.as_array()?;
    let mut programs = ArrayOfTables::new();

    for value in array.iter() {
        let table = value.as_inline_table()?.clone().into_table();
        programs.push(table);
    }

    Some(programs)
}

fn program_positions(programs: &ArrayOfTables) -> Vec<usize> {
    let mut positions: Vec<usize> = programs.iter().filter_map(Table::position).collect();
    positions.sort_unstable();
    positions
}

fn set_programs(doc: &mut DocumentMut, reordered: Vec<Table>, positions: Vec<usize>) {
    let positions = reassigned_positions(&positions, reordered.len());
    let mut programs = ArrayOfTables::new();
    for (index, mut program) in reordered.into_iter().enumerate() {
        if let Some(position) = positions.get(index) {
            program.set_position(*position);
        }
        programs.push(program);
    }
    doc["programs"] = Item::ArrayOfTables(programs);
}

fn reassigned_positions(existing: &[usize], len: usize) -> Vec<usize> {
    match existing {
        [] => (0..len).collect(),
        positions if positions.len() == len => positions.to_vec(),
        positions => (positions[0]..positions[0] + len).collect(),
    }
}

fn program_table(command: &str) -> Table {
    let mut table = Table::new();
    table["label"] = toml_edit::value(command);
    table["command"] = toml_edit::value(command);
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn migrate(src: &str) -> String {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, src).unwrap();
        migrate_config_file(&path).unwrap().unwrap()
    }

    #[test]
    fn missing_file_is_unchanged() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            migrate_config_file(&dir.path().join("config.toml")).unwrap(),
            None
        );
    }

    #[test]
    fn default_program_only_becomes_first_program() {
        let out = migrate("default_program = \"codex --full-auto\"\n");
        assert!(!out.contains("default_program"));
        assert!(out.contains("[[programs]]"));
        assert!(out.contains("label = \"codex --full-auto\""));
        assert!(out.contains("command = \"codex --full-auto\""));
    }

    #[test]
    fn empty_inline_programs_migrates_with_clean_table_header() {
        let out = migrate("default_program = \"codex\"\nprograms = []\n");
        assert!(out.contains("[[programs]]"));
        assert!(!out.contains("[[programs ]]"));
    }

    #[test]
    fn matching_program_moves_to_front() {
        let out = migrate(
            r#"default_program = "codex"

[[programs]]
label = "Claude"
command = "claude"

[[programs]]
label = "Codex"
command = "codex"
"#,
        );
        assert!(!out.contains("default_program"));
        assert!(
            out.find("command = \"codex\"").unwrap() < out.find("command = \"claude\"").unwrap()
        );
    }

    #[test]
    fn matching_programs_stay_adjacent_after_unrelated_table() {
        let out = migrate(
            r#"default_program = "codex"

[keybindings]
new_session = ["n"]

[[programs]]
label = "Claude"
command = "claude"

[[programs]]
label = "Codex"
command = "codex"
"#,
        );

        let keybindings = out.find("[keybindings]").unwrap();
        let codex = out.find("command = \"codex\"").unwrap();
        let claude = out.find("command = \"claude\"").unwrap();
        assert!(keybindings < codex);
        assert!(codex < claude);
        assert_eq!(out[codex..claude].matches("[[programs]]").count(), 1);
    }

    #[test]
    fn non_matching_program_inserts_first() {
        let out = migrate(
            r#"default_program = "aider"

[[programs]]
label = "Claude"
command = "claude"
"#,
        );
        assert!(
            out.find("command = \"aider\"").unwrap() < out.find("command = \"claude\"").unwrap()
        );
    }

    #[test]
    fn migrated_config_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"[[programs]]
label = "Claude"
command = "claude"
"#,
        )
        .unwrap();
        assert_eq!(
            migrate_config_file(&path).unwrap().unwrap(),
            std::fs::read_to_string(path).unwrap()
        );
    }
}
