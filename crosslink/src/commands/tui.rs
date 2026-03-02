use anyhow::Result;

use crate::db::Database;

pub fn run(db: &Database) -> Result<()> {
    crate::tui::run(db)
}
