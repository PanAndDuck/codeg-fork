use codeg_lib::{flavor, paths};

#[test]
fn explicit_test_home_covers_every_managed_root() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    temp_env::with_vars(
        [
            ("CODEG_HOME", Some(home.path().as_os_str())),
            ("CODEG_DATA_DIR", Some(data.path().as_os_str())),
        ],
        || {
            assert_eq!(paths::codeg_home_dir(), home.path());
            assert_eq!(paths::user_codeg_home_dir().unwrap(), home.path());
            for (path, name) in [
                (paths::codeg_pets_root(), "pets"),
                (paths::codeg_browser_profiles_root(), "browser-profiles"),
                (paths::codeg_uploads_root(), "uploads"),
                (paths::codeg_backgrounds_root(), "backgrounds"),
                (paths::codeg_logs_root(), "logs"),
                (paths::codeg_turn_timings_root(), "turn-timings"),
                (paths::codeg_acp_transcripts_root(), "acp-transcripts"),
            ] {
                assert_eq!(path, home.path().join(name));
            }
            #[cfg(feature = "tauri-runtime")]
            assert_eq!(
                codeg_lib::preferences::preferences_file_path().unwrap(),
                home.path().join("preferences.json")
            );
        },
    );
}

#[test]
fn default_roots_use_the_compiled_flavor_without_reading_state() {
    temp_env::with_vars_unset(["CODEG_HOME", "CODEG_DATA_DIR"], || {
        let expected = dirs::home_dir()
            .unwrap()
            .join(flavor::current().home_dir_name());
        assert_eq!(paths::codeg_home_dir(), expected);
        assert_eq!(paths::codeg_logs_root(), expected.join("logs"));
        assert_eq!(paths::codeg_uploads_root(), expected.join("uploads"));
    });
}

#[tokio::test]
async fn disabled_update_check_is_local_and_cannot_fetch_official_manifest() {
    if flavor::current().updater_enabled() {
        return;
    }
    let result = codeg_lib::web::handlers::web_server::check_app_update()
        .await
        .unwrap();
    let value = serde_json::to_value(result.0).unwrap();
    assert!(value["update"].is_null());
    assert_eq!(value["selfUpdateSupported"], false);
    assert!(codeg_lib::update::version::fetch_latest_manifest()
        .await
        .is_err());
}

#[tokio::test]
async fn fork_global_skill_entrypoints_cannot_read_or_import_shared_skills() {
    if flavor::current() != flavor::Flavor::Fork {
        return;
    }
    let agent = codeg_lib::models::AgentType::Codex;
    let listed = codeg_lib::commands::acp::acp_list_agent_skills(agent, None)
        .await
        .unwrap();
    assert!(listed.locations.is_empty());
    assert!(listed.skills.is_empty());
    assert!(
        codeg_lib::commands::custom_skills::custom_import_from_agent(
            agent,
            vec!["test-skill".into()]
        )
        .await
        .is_err()
    );
}
