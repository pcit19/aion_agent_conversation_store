use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct Turn {
    pub id: i64,
    pub conversation_id: Uuid,
    pub workspace_id: Uuid,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnView {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

impl From<Turn> for TurnView {
    fn from(t: Turn) -> Self {
        Self {
            id: t.id,
            role: t.role,
            content: t.content,
            created_at: t.created_at,
        }
    }
}

/// Trims a chronological list of turns to fit within `max_chars`.
/// Takes newest turns backwards until limit is reached, maintaining ascending chronological order.
pub fn trim_turns(mut turns: Vec<Turn>, max_chars: usize) -> Vec<Turn> {
    if turns.is_empty() {
        return turns;
    }

    let total: usize = turns.iter().map(|t| t.content.len()).sum();
    if total <= max_chars {
        return turns;
    }

    let mut current_chars = 0;
    let mut keep_from_idx = turns.len();

    for (idx, turn) in turns.iter().enumerate().rev() {
        let turn_len = turn.content.len();
        if current_chars + turn_len > max_chars && current_chars > 0 {
            break;
        }
        current_chars += turn_len;
        keep_from_idx = idx;
    }

    turns.split_off(keep_from_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_turn(id: i64, content: &str) -> Turn {
        Turn {
            id,
            conversation_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            role: "user".into(),
            content: content.into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_trim_empty() {
        let trimmed = trim_turns(vec![], 1000);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_trim_under_limit() {
        let turns = vec![
            dummy_turn(1, "hello"),
            dummy_turn(2, "world"),
        ];
        let trimmed = trim_turns(turns.clone(), 100);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].id, 1);
        assert_eq!(trimmed[1].id, 2);
    }

    #[test]
    fn test_trim_exceeding_drops_oldest_first() {
        let turns = vec![
            dummy_turn(1, "oldest turn that should be dropped"),
            dummy_turn(2, "middle turn"),
            dummy_turn(3, "newest turn"),
        ];
        // "middle turn" (11 chars) + "newest turn" (11 chars) = 22 chars
        let trimmed = trim_turns(turns, 25);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].id, 2);
        assert_eq!(trimmed[1].id, 3);
    }

    #[test]
    fn test_trim_keeps_at_least_newest_turn_even_if_huge() {
        let turns = vec![
            dummy_turn(1, "short"),
            dummy_turn(2, "a very long turn exceeding limit by itself"),
        ];
        let trimmed = trim_turns(turns, 10);
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].id, 2);
    }
}
