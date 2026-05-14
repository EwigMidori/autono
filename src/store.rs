use std::fs;
use std::path::Path;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Result, ResultContext};
use crate::workflow::ManagedState;

#[non_exhaustive]
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredItem {
    pub owner: String,
    pub repo: String,
    pub item_id: String,
    pub state: ManagedState,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub pr_number: Option<i64>,
    pub last_comment_id: Option<i64>,
    pub last_review_id: Option<i64>,
    pub updated_at: OffsetDateTime,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create state dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open state database {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn get_item(&self, owner: &str, repo: &str, item_id: &str) -> Result<Option<StoredItem>> {
        self.conn
            .query_row(
                "select owner, repo, item_id, state, branch, worktree_path, pr_number, last_comment_id, last_review_id, updated_at
                 from items where owner = ?1 and repo = ?2 and item_id = ?3",
                params![owner, repo, item_id],
                StoredItem::from_row,
            )
            .optional()
            .context("failed to load stored item")
    }

    pub fn upsert_item(&self, item: &StoredItem) -> Result<()> {
        self.conn.execute(
            "insert into items
             (owner, repo, item_id, state, branch, worktree_path, pr_number, last_comment_id, last_review_id, updated_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             on conflict(owner, repo, item_id) do update set
               state = excluded.state,
               branch = excluded.branch,
               worktree_path = excluded.worktree_path,
               pr_number = excluded.pr_number,
               last_comment_id = excluded.last_comment_id,
               last_review_id = excluded.last_review_id,
               updated_at = excluded.updated_at",
            params![
                item.owner,
                item.repo,
                item.item_id,
                item.state.to_string(),
                item.branch,
                item.worktree_path,
                item.pr_number,
                item.last_comment_id,
                item.last_review_id,
                item.updated_at.format(&Rfc3339)?,
            ],
        )?;
        Ok(())
    }

    pub fn mark_state(
        &self,
        owner: &str,
        repo: &str,
        item_id: &str,
        state: ManagedState,
        last_comment_id: Option<i64>,
    ) -> Result<StoredItem> {
        let existing = self.get_item(owner, repo, item_id)?;
        let mut item = existing.unwrap_or_else(|| StoredItem {
            owner: owner.to_string(),
            repo: repo.to_string(),
            item_id: item_id.to_string(),
            state,
            branch: None,
            worktree_path: None,
            pr_number: None,
            last_comment_id,
            last_review_id: None,
            updated_at: OffsetDateTime::now_utc(),
        });
        item.state = state;
        if let Some(last_comment_id) = last_comment_id {
            item.last_comment_id = Some(last_comment_id);
        }
        item.updated_at = OffsetDateTime::now_utc();
        self.upsert_item(&item)?;
        Ok(item)
    }

    pub fn attach_work(
        &self,
        owner: &str,
        repo: &str,
        item_id: &str,
        branch: &str,
        worktree_path: &str,
    ) -> Result<StoredItem> {
        let mut item = self
            .get_item(owner, repo, item_id)?
            .unwrap_or_else(|| StoredItem::new(owner, repo, item_id, ManagedState::Working));
        item.state = ManagedState::Working;
        item.branch = Some(branch.to_string());
        item.worktree_path = Some(worktree_path.to_string());
        item.updated_at = OffsetDateTime::now_utc();
        self.upsert_item(&item)?;
        Ok(item)
    }

    pub fn attach_pr(
        &self,
        owner: &str,
        repo: &str,
        item_id: &str,
        pr_number: i64,
    ) -> Result<StoredItem> {
        let mut item = self
            .get_item(owner, repo, item_id)?
            .unwrap_or_else(|| StoredItem::new(owner, repo, item_id, ManagedState::PrOpen));
        item.state = ManagedState::PrOpen;
        item.pr_number = Some(pr_number);
        item.updated_at = OffsetDateTime::now_utc();
        self.upsert_item(&item)?;
        Ok(item)
    }

    pub fn mark_review_handled(
        &self,
        owner: &str,
        repo: &str,
        item_id: &str,
        review_id: Option<i64>,
    ) -> Result<StoredItem> {
        let mut item = self
            .get_item(owner, repo, item_id)?
            .unwrap_or_else(|| StoredItem::new(owner, repo, item_id, ManagedState::ReviewPending));
        item.last_review_id = review_id;
        item.updated_at = OffsetDateTime::now_utc();
        self.upsert_item(&item)?;
        Ok(item)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "create table if not exists items (
                owner text not null,
                repo text not null,
                item_id text not null,
                state text not null,
                branch text,
                worktree_path text,
                pr_number integer,
                last_comment_id integer,
                last_review_id integer,
                updated_at text not null,
                primary key (owner, repo, item_id)
            );",
        )?;
        self.conn
            .execute("alter table items add column last_review_id integer", [])
            .or_else(|err| match err {
                rusqlite::Error::SqliteFailure(_, ref message)
                    if message
                        .as_deref()
                        .map(|message| message.contains("duplicate column name"))
                        .unwrap_or(false) =>
                {
                    Ok(0)
                }
                err => Err(err),
            })?;
        Ok(())
    }
}

impl StoredItem {
    pub fn new(owner: &str, repo: &str, item_id: &str, state: ManagedState) -> Self {
        Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            item_id: item_id.to_string(),
            state,
            branch: None,
            worktree_path: None,
            pr_number: None,
            last_comment_id: None,
            last_review_id: None,
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let state_raw: String = row.get(3)?;
        let updated_at_raw: String = row.get(9)?;
        let state = ManagedState::from_str(&state_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
        })?;
        let updated_at = OffsetDateTime::parse(&updated_at_raw, &Rfc3339).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(err))
        })?;

        Ok(Self {
            owner: row.get(0)?,
            repo: row.get(1)?,
            item_id: row.get(2)?,
            state,
            branch: row.get(4)?,
            worktree_path: row.get(5)?,
            pr_number: row.get(6)?,
            last_comment_id: row.get(7)?,
            last_review_id: row.get(8)?,
            updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrips_item() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = Store::open(tempdir.path().join("state.sqlite3")).unwrap();
        let item = StoredItem {
            owner: "o".to_string(),
            repo: "r".to_string(),
            item_id: "i".to_string(),
            state: ManagedState::Working,
            branch: Some("agent/i".to_string()),
            worktree_path: Some("/tmp/w".to_string()),
            pr_number: Some(42),
            last_comment_id: Some(7),
            last_review_id: Some(9),
            updated_at: OffsetDateTime::now_utc(),
        };
        store.upsert_item(&item).unwrap();
        let loaded = store.get_item("o", "r", "i").unwrap().unwrap();
        assert_eq!(loaded.state, ManagedState::Working);
        assert_eq!(loaded.branch.as_deref(), Some("agent/i"));
        assert_eq!(loaded.pr_number, Some(42));
        assert_eq!(loaded.last_review_id, Some(9));
    }
}
