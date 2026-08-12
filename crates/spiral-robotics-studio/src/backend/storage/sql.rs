//! Backend-only SQL construction and authorization for SpiralDB.
//!
//! Owned relation and column names come from `spiral_robotics_core::spiral`. Keeping
//! string literals and dynamic-resource naming here makes every generated
//! statement auditable while leaving raw user queries behind one read-only
//! gate.

use spiral_robotics_core::spiral::{self, Table, column};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadOnlyMode {
    Preview,
    Training,
}

pub(crate) fn relation(dataset: &str, table: Table) -> String {
    spiral::relation(dataset, table)
}

pub(crate) fn count_rows(relation: &str) -> String {
    format!("SELECT COUNT(*) AS rows FROM {relation}")
}

#[cfg(test)]
pub(crate) fn count_episode_rows(relation: &str, episode_id: &str) -> String {
    format!(
        "SELECT COUNT(*) AS rows FROM {relation} WHERE {} = {}",
        column::EPISODE_ID,
        string_literal(episode_id)
    )
}

pub(crate) fn episode_exists(relation: &str, episode_id: &str) -> String {
    format!(
        "SELECT {episode_id_column} FROM {relation} WHERE {episode_id_column} = {episode_id} LIMIT 1",
        episode_id_column = column::EPISODE_ID,
        episode_id = string_literal(episode_id),
    )
}

pub(crate) fn delete_episode(relation: &str, episode_id: &str) -> String {
    format!(
        "DELETE FROM {relation} WHERE {} = {}",
        column::EPISODE_ID,
        string_literal(episode_id)
    )
}

pub(crate) fn trajectory(dataset: &str, episode_id: &str, stream_id: &str) -> String {
    let episodes = relation(dataset, Table::Episodes);
    let streams = relation(dataset, Table::EpisodeStreams);
    let samples = relation(dataset, Table::JointSamples);
    format!(
        "SELECT e.{sample_hz}, s.{joint_names}, \
         COALESCE(j.{action}, j.{state}) AS {replay} \
         FROM {episodes} e JOIN {streams} s ON e.{episode_id} = s.{episode_id} \
         JOIN {samples} j ON j.{episode_id} = s.{episode_id} AND j.{stream_id} = s.{stream_id} \
         WHERE e.{episode_id} = {episode_value} AND s.{stream_id} = {stream_value} \
         ORDER BY j.{sample_index}",
        sample_hz = column::SAMPLE_HZ,
        joint_names = column::JOINT_NAMES,
        action = column::ACTION_POSITION,
        state = column::STATE_POSITION,
        replay = column::REPLAY_POSITION,
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        sample_index = column::SAMPLE_INDEX,
        episode_value = string_literal(episode_id),
        stream_value = string_literal(stream_id),
    )
}

pub(crate) fn video_asset(
    dataset: &str,
    episode_id: &str,
    stream_id: &str,
    segment: Option<u32>,
) -> String {
    let assets = relation(dataset, Table::VideoAssets);
    let segment_filter = segment
        .map(|segment| format!(" AND {} = {segment}", column::SEGMENT_INDEX))
        .unwrap_or_default();
    format!(
        "SELECT {payload} FROM {assets} WHERE {episode_id_column} = {episode_id} \
         AND {stream_id_column} = {stream_id}{segment_filter} \
         ORDER BY {segment_index} LIMIT 1",
        payload = column::PAYLOAD,
        episode_id_column = column::EPISODE_ID,
        episode_id = string_literal(episode_id),
        stream_id_column = column::STREAM_ID,
        stream_id = string_literal(stream_id),
        segment_index = column::SEGMENT_INDEX,
    )
}

pub(crate) fn create_dataset_namespace(dataset: &str) -> String {
    format!(
        "CREATE TABLE {dataset} AS SELECT {} AS {}",
        string_literal(dataset),
        spiral::DATASET_NAMESPACE_MARKER
    )
}

pub(crate) fn video_sources_relation(dataset: &str, episode_id: &str, managed: bool) -> String {
    if managed {
        format!(
            "{}_{}_video_sources",
            dataset,
            spiral::identifier_component(episode_id)
        )
    } else {
        format!("{dataset}.video_sources")
    }
}

pub(crate) fn video_dataset_relation(dataset: &str, episode_id: &str) -> String {
    format!(
        "{dataset}.video_{}",
        spiral::identifier_component(episode_id)
    )
}

pub(crate) fn video_dataset_output(dataset: &str, episode_id: &str) -> String {
    format!("{}.assets", video_dataset_relation(dataset, episode_id))
}

pub(crate) fn create_video_sources(
    sources: &str,
    assets: &str,
    episode_id: Option<&str>,
) -> String {
    let source_filter = episode_id
        .map(|episode_id| {
            format!(
                " WHERE assets.{} = {}",
                column::EPISODE_ID,
                string_literal(episode_id)
            )
        })
        .unwrap_or_default();
    format!(
        "CREATE OR REPLACE VIEW {sources} AS SELECT assets.{episode_id}, assets.{stream_id}, \
         assets.{segment_index}, assets.{timeline_offset}, \
         spql.blob.inline(assets.{payload}, assets.{byte_size}, 'video/mp4') AS {source} \
         FROM {assets} AS assets{source_filter}",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        segment_index = column::SEGMENT_INDEX,
        timeline_offset = column::TIMELINE_OFFSET_NS,
        payload = column::PAYLOAD,
        byte_size = column::BYTE_SIZE,
        source = column::SOURCE,
    )
}

pub(crate) fn create_video_dataset(dataset: &str, sources: &str) -> String {
    format!(
        "CREATE DATASET {dataset} USING spql.mp4.dataset FROM {sources} \
         WITH (blob = '{}', index_tier = 'dependency')",
        column::SOURCE
    )
}

#[cfg(test)]
pub(crate) fn video_tracks(sources: &str, episode_id: &str) -> String {
    format!(
        "SELECT v.{episode_id}, v.{stream_id}, tracks.* FROM {sources} v \
         CROSS JOIN LATERAL spql.mp4.tracks(v.{source}) AS tracks \
         WHERE v.{episode_id} = {episode_value}",
        episode_id = column::EPISODE_ID,
        stream_id = column::STREAM_ID,
        source = column::SOURCE,
        episode_value = string_literal(episode_id),
    )
}

pub(crate) fn refresh_dataset(dataset: &str) -> String {
    format!("REFRESH DATASET {dataset}")
}

pub(crate) fn ensure_read_only(sql: &str, mode: ReadOnlyMode) -> Result<(), String> {
    let sql = sql.trim();
    let label = match mode {
        ReadOnlyMode::Preview => "Spiral preview",
        ReadOnlyMode::Training => "Training SQL",
    };
    if sql.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    if sql.len() > 64 * 1024 {
        return Err(format!("{label} must be under 64 KiB"));
    }
    let (tokens, semicolons) = sql_tokens(sql)?;
    if semicolons > 1 {
        return Err(format!("{label} must contain exactly one read-only query"));
    }
    let first = tokens.first().map(String::as_str).unwrap_or_default();
    let allowed = match mode {
        ReadOnlyMode::Preview => matches!(first, "SELECT" | "WITH" | "EXPLAIN"),
        ReadOnlyMode::Training => matches!(first, "SELECT" | "WITH"),
    };
    if !allowed {
        return Err(match mode {
            ReadOnlyMode::Preview => {
                "Spiral preview accepts SELECT, WITH, or EXPLAIN queries only".to_owned()
            }
            ReadOnlyMode::Training => {
                "Training SQL must be a read-only SELECT or WITH query".to_owned()
            }
        });
    }
    const MUTATING_KEYWORDS: &[&str] = &[
        "ALTER", "ATTACH", "CALL", "COPY", "CREATE", "DELETE", "DETACH", "DROP", "EXECUTE",
        "GRANT", "INSERT", "MERGE", "REFRESH", "REPLACE", "REVOKE", "TRUNCATE", "UPDATE",
    ];
    if let Some(keyword) = tokens
        .iter()
        .find(|token| MUTATING_KEYWORDS.contains(&token.as_str()))
    {
        return Err(format!(
            "{label} is read-only; {:?} is not allowed",
            keyword.to_ascii_lowercase()
        ));
    }
    Ok(())
}

/// Escape a value for the SQL string-literal grammar. ADBC parameters are not
/// uniformly implemented by the Spiral drivers currently supported here, so
/// generated statements isolate this compatibility fallback in one function.
fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Extract unquoted SQL words while rejecting extra statements. The driver's
/// parser remains authoritative; this is a conservative authorization gate.
fn sql_tokens(sql: &str) -> Result<(Vec<String>, usize), String> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        LineComment,
        BlockComment,
    }

    let bytes = sql.as_bytes();
    let mut state = State::Normal;
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut semicolons = 0;
    let mut statement_token_count = None;
    let mut index = 0;
    let flush = |token: &mut String, tokens: &mut Vec<String>| {
        if !token.is_empty() {
            tokens.push(std::mem::take(token).to_ascii_uppercase());
        }
    };
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Normal => match (byte, next) {
                (b'-', Some(b'-')) => {
                    flush(&mut token, &mut tokens);
                    state = State::LineComment;
                    index += 1;
                }
                (b'/', Some(b'*')) => {
                    flush(&mut token, &mut tokens);
                    state = State::BlockComment;
                    index += 1;
                }
                (b'\'', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::SingleQuote;
                }
                (b'"', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::DoubleQuote;
                }
                (b';', _) => {
                    flush(&mut token, &mut tokens);
                    semicolons += 1;
                    statement_token_count.get_or_insert(tokens.len());
                }
                _ if byte.is_ascii_alphanumeric() || byte == b'_' => {
                    token.push(char::from(byte));
                }
                _ => flush(&mut token, &mut tokens),
            },
            State::SingleQuote => {
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::LineComment => {
                if matches!(byte, b'\n' | b'\r') {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    state = State::Normal;
                    index += 1;
                }
            }
        }
        index += 1;
    }
    flush(&mut token, &mut tokens);
    if !matches!(state, State::Normal | State::LineComment) {
        return Err("Spiral SQL contains an unterminated quote or comment".to_owned());
    }
    if statement_token_count.is_some_and(|count| tokens.len() != count) {
        return Err("Spiral SQL must contain exactly one read-only query".to_owned());
    }
    Ok((tokens, semicolons))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_values_are_escaped_in_one_place() {
        let sql = episode_exists("robot.episodes", "take-'one");
        assert!(sql.contains("'take-''one'"));
    }

    #[test]
    fn generated_training_relations_follow_the_shared_contract() {
        let sql = trajectory("robotics", "episode-1", "leader");
        for table in [Table::Episodes, Table::EpisodeStreams, Table::JointSamples] {
            assert!(sql.contains(&relation("robotics", table)));
        }
    }

    #[test]
    fn read_only_gate_handles_comments_quotes_and_cte_mutation() {
        assert!(
            ensure_read_only("SELECT ';' AS punctuation; -- okay", ReadOnlyMode::Preview).is_ok()
        );
        assert!(
            ensure_read_only(
                "WITH recent AS (SELECT 1) SELECT * FROM recent",
                ReadOnlyMode::Training
            )
            .is_ok()
        );
        assert!(
            ensure_read_only(
                "WITH removed AS (DELETE FROM t) SELECT 1",
                ReadOnlyMode::Training
            )
            .is_err()
        );
        assert!(ensure_read_only("SELECT 1; DROP TABLE t", ReadOnlyMode::Preview).is_err());
    }
}
