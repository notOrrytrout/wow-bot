use std::{env, str::FromStr, time::Duration};

use sqlx::{
    MySqlPool,
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
};

const BOT_MEMORY_TABLES: &[&str] = &[
    "planner_state",
    "agreed_player_plans",
    "conversation_messages",
    "action_runs",
    "memory_facts",
    "spell_usage",
    "action_failures",
    "tool_usage",
    "learned_facts",
    "learned_fact_events",
    "mob_death_risks",
];
const BOT_MEMORY_CLEAR_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn clear_bot_memory(database_url_env: &str, bot_id: &str) -> Result<(), String> {
    let bot_id = bot_id.trim();
    if bot_id.is_empty() {
        return Err("cannot clear persistent memory without a configured bot identity".into());
    }
    let database_url = env::var(database_url_env).ok();
    let database_url = configured_database_url(database_url_env, database_url.as_deref())?;
    let options = MySqlConnectOptions::from_str(&database_url)
        .map_err(|_| "persistent memory database URL is invalid".to_owned())?;
    validate_connection_security(&options)?;
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy_with(options);
    tokio::time::timeout(
        BOT_MEMORY_CLEAR_TIMEOUT,
        clear_bot_memory_with_pool(&pool, bot_id),
    )
    .await
    .map_err(|_| {
        "persistent memory clear timed out; check database state before retrying".to_owned()
    })?
}

fn validate_connection_security(options: &MySqlConnectOptions) -> Result<(), String> {
    let verified_tls = matches!(options.get_ssl_mode(), MySqlSslMode::VerifyIdentity);
    crate::memory::remote::validate_remote_endpoint(
        options.get_host(),
        verified_tls,
        verified_tls,
        &crate::memory::remote::RemoteMemoryPolicy::default(),
    )
}

fn configured_database_url<'a>(env_name: &str, value: Option<&'a str>) -> Result<&'a str, String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("persistent memory is unavailable: environment variable {env_name} is not set")
        })?;
    Ok(value)
}

async fn clear_bot_memory_with_pool(pool: &MySqlPool, bot_id: &str) -> Result<(), String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| "could not start the persistent memory clear transaction".to_owned())?;
    for table in BOT_MEMORY_TABLES {
        let statement = delete_statement(table);
        sqlx::query(&statement)
            .bind(bot_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| format!("could not clear persistent memory table {table}"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| "could not commit the persistent memory clear transaction".to_owned())
}

fn delete_statement(table: &str) -> String {
    format!("DELETE FROM {table} WHERE bot_id = ?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_database_environment_variable_fails_closed_without_connecting() {
        let error = configured_database_url("TEST_TENTACLI_MEMORY_URL", None).unwrap_err();
        assert!(error.contains("TEST_TENTACLI_MEMORY_URL"));
        assert!(error.contains("not set"));
    }

    #[test]
    fn empty_database_environment_value_fails_closed_without_connecting() {
        assert!(configured_database_url("TEST_URL", Some("  ")).is_err());
    }

    #[test]
    fn clear_uses_only_static_tables_and_bot_bound_parameters() {
        assert_eq!(
            BOT_MEMORY_TABLES,
            &[
                "planner_state",
                "agreed_player_plans",
                "conversation_messages",
                "action_runs",
                "memory_facts",
                "spell_usage",
                "action_failures",
                "tool_usage",
                "learned_facts",
                "learned_fact_events",
                "mob_death_risks",
            ]
        );
        assert!(BOT_MEMORY_TABLES.iter().all(|table| {
            table
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                && delete_statement(table) == format!("DELETE FROM {table} WHERE bot_id = ?")
        }));
        assert_eq!(
            delete_statement("planner_state"),
            "DELETE FROM planner_state WHERE bot_id = ?"
        );
    }

    #[test]
    fn non_loopback_database_requires_verified_identity_tls() {
        let insecure =
            MySqlConnectOptions::from_str("mysql://bot:pass@db.example.com/bot_memory").unwrap();
        assert!(validate_connection_security(&insecure).is_err());

        let secure = MySqlConnectOptions::from_str(
            "mysql://bot:pass@db.example.com/bot_memory?ssl-mode=verify_identity",
        )
        .unwrap();
        assert!(validate_connection_security(&secure).is_ok());
    }
}
