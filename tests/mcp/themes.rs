//! The `themes` tool group as seen by a real MCP client:
//! `get_texture_theme` resolving against the fixture IWAD's thin texture
//! set, `build_texture` names being preferred once they exist,
//! `set_texture_theme` switching themes and validating overrides,
//! `auto_texture` filling the fixture's lower gap and repairing a broken
//! flat, `align_textures` running the fixture's hall wall as one chain,
//! and every error shape along the way.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_heretic};
    use crate::support::{call, call_err, connect};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// Open the `DOOM II` fixture (`WALL1`/`STEP1` textures, `FLOOR1`
    /// flat, one two-room `MAP01`) as the base layer.
    async fn doom_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// The theme view for the client's current session.
    async fn theme(client: &Client) -> Value {
        call(client, "get_texture_theme", Value::Null).await
    }

    /// The `{texture, matched}` pair reported for `role`.
    async fn role_view(client: &Client, role: &str) -> Value {
        theme(client).await["roles"][role].clone()
    }

    /// Asserts `tool` rejects `args` with the error code `want`.
    async fn refuse(client: &Client, tool: &str, args: Value, want: &str) {
        let note = args.to_string();
        let (code, _, _) = call_err(client, tool, args).await;
        assert_eq!(code, want, "refusal code for {tool} {note}");
    }

    /// Define a theme texture (patch `WALLP1`, 64 wide) through
    /// `build_texture`.
    async fn build(client: &Client, name: &str) {
        call(
            client,
            "build_texture",
            json!({
                "name": name,
                "width": 64_u64,
                "height": 128_u64,
                "patches": [{ "patch": "WALLP1", "x": 0_i64, "y": 0_i64 }],
            }),
        )
        .await;
    }

    /// The working archive's sidedef rows for `map`, in order.
    async fn working_sidedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["sidedefs"]}),
        )
        .await;
        report["sidedefs"]
            .as_array()
            .expect("sidedefs is an array")
            .clone()
    }

    #[tokio::test]
    async fn an_empty_session_offers_the_doom_themes_and_no_textures() {
        let client = connect().await;
        let view = theme(&client).await;
        assert_eq!(view["theme"], json!("tech"), "unknown game follows doom");
        assert_eq!(
            view["available_themes"],
            json!(["brick", "hell", "rock", "tech"]),
            "the doom table's themes"
        );
        assert_eq!(
            view["roles"]["wall"],
            json!({"texture": "-", "matched": "default"}),
            "the session holds nothing of any kind"
        );
    }

    #[tokio::test]
    async fn the_fixture_resolves_by_glob_default_and_the_always_open_sky() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        assert_eq!(
            role_view(&client, "floor").await,
            json!({"texture": "FLOOR1", "matched": "glob"}),
            "the FLOOR* candidate finds the fixture flat"
        );
        assert_eq!(
            role_view(&client, "wall").await,
            json!({"texture": "STEP1", "matched": "default"}),
            "no STARTAN3 anywhere: the first available wall wins"
        );
        assert_eq!(
            role_view(&client, "outdoor_ceiling").await,
            json!({"texture": "F_SKY1", "matched": "exact"}),
            "the sky is always allowed"
        );
        assert_eq!(
            role_view(&client, "light_ceiling").await,
            json!({"texture": "FLOOR1", "matched": "default"}),
            "the lit ceiling falls back to the ceiling row, which misses too, so the game
             default path ends at the first flat"
        );
        assert_eq!(
            role_view(&client, "ceiling").await,
            json!({"texture": "FLOOR1", "matched": "default"}),
            "no CEIL* flat exists here"
        );
    }

    #[tokio::test]
    async fn added_theme_textures_are_chosen_by_the_same_roles() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        build(&client, "STARTAN3").await;
        build(&client, "DOORTRAK").await;
        assert_eq!(
            role_view(&client, "wall").await,
            json!({"texture": "STARTAN3", "matched": "exact"}),
            "the table's first exact name landed"
        );
        assert_eq!(
            role_view(&client, "door_track").await,
            json!({"texture": "DOORTRAK", "matched": "exact"})
        );
        assert_eq!(
            role_view(&client, "door").await,
            json!({"texture": "DOORTRAK", "matched": "glob"}),
            "the *DOOR* glob now finds the track"
        );
        assert_eq!(
            role_view(&client, "door_track_red").await,
            json!({"texture": "DOORTRAK", "matched": "fallback"}),
            "DOORRED is missing, so the chain walks to door_track"
        );
    }

    #[tokio::test]
    async fn themes_switch_and_overrides_replace_or_clear() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let switched = call(&client, "set_texture_theme", json!({"theme": "HELL"})).await;
        assert_eq!(switched["theme"], json!("hell"), "case-insensitive hit");
        assert_eq!(
            switched["roles"]["wall"]["texture"],
            json!("STEP1"),
            "hell walls (MARBLE1 ...) miss and the default wall takes over"
        );
        assert_eq!(
            switched["roles"]["door"]["texture"],
            json!("STEP1"),
            "WOODMET1 and DOOR3 are not in the fixture either"
        );
        let overridden = call(
            &client,
            "set_texture_theme",
            json!({"overrides": {"wall": "WALL1", "floor": "FLOOR1"}}),
        )
        .await;
        assert_eq!(
            overridden["roles"]["wall"],
            json!({"texture": "WALL1", "matched": "exact"}),
            "the override wins over the theme"
        );
        let cleared = call(
            &client,
            "set_texture_theme",
            json!({"overrides": {"wall": "-"}}),
        )
        .await;
        assert_eq!(
            cleared["roles"]["wall"]["matched"],
            json!("default"),
            "clearing returns the role to the theme"
        );
    }

    /// Requires a theme refusal `message` to echo the wanted name and list
    /// every theme the `DOOM II` fixture's table offers.
    fn refusal_lists_the_choices(message: &str, wanted: &str) {
        assert!(message.contains(wanted), "names {wanted}: {message}");
        for name in ["brick", "hell", "rock", "tech"] {
            assert!(
                message.contains(name),
                "the refusal lists {name}: {message}"
            );
        }
    }

    #[tokio::test]
    async fn set_texture_theme_refuses_noise() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message, hint) =
            call_err(&client, "set_texture_theme", json!({"theme": "nope"})).await;
        assert_eq!(code, "invalid_argument", "unknown theme");
        refusal_lists_the_choices(&message, "nope");
        assert!(
            hint.contains("get_texture_theme"),
            "the hint points at the listing tool: {hint}"
        );
        let (spelt_code, spelt_message, _) =
            call_err(&client, "set_texture_theme", json!({"theme": "tech_base"})).await;
        assert_eq!(
            spelt_code, "invalid_argument",
            "a misspelt theme is refused, not defaulted"
        );
        refusal_lists_the_choices(&spelt_message, "tech_base");
        refuse(
            &client,
            "set_texture_theme",
            json!({"overrides": {"wall": "NOPE"}}),
            "not_found",
        )
        .await;
        refuse(&client, "set_texture_theme", json!({}), "invalid_argument").await;
    }

    #[tokio::test]
    async fn heretic_follows_its_own_table() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_heretic(dir.path());
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        let view = theme(&client).await;
        assert_eq!(view["theme"], json!("castle"), "the heretic default");
        assert_eq!(view["available_themes"], json!(["castle"]));
    }

    #[tokio::test]
    async fn a_theme_that_stops_matching_the_game_is_refused_not_defaulted() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "set_texture_theme", json!({"theme": "hell"})).await;
        let heretic = iwad_heretic(dir.path());
        call(
            &client,
            "open_iwad",
            json!({"path": heretic.display().to_string()}),
        )
        .await;
        let (code, message, hint) = call_err(&client, "get_texture_theme", Value::Null).await;
        assert_eq!(
            code, "invalid_argument",
            "the doom theme is not a heretic one and does not silently become castle"
        );
        assert!(
            message.contains("hell") && message.contains("castle"),
            "the refusal names the stale theme and the game's choices: {message}"
        );
        assert!(
            hint.contains("get_texture_theme"),
            "the hint points at the listing tool: {hint}"
        );
        let healed = call(&client, "set_texture_theme", json!({"theme": "castle"})).await;
        assert_eq!(
            healed["theme"],
            json!("castle"),
            "a listed name clears the refusal"
        );
    }

    #[tokio::test]
    async fn tx_lumps_count_as_wall_textures() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "add_lump",
            json!({"name": "TX_ZEPPL", "text": "not really a texture, just bytes",
                   "namespace": "textures"}),
        )
        .await;
        let view = call(
            &client,
            "set_texture_theme",
            json!({"overrides": {"wall": "ZEPPL"}}),
        )
        .await;
        assert_eq!(
            view["roles"]["wall"],
            json!({"texture": "ZEPPL", "matched": "exact"}),
            "the TX_ prefix strips for the wall namespace"
        );
    }

    #[tokio::test]
    async fn auto_texture_fill_dresses_the_gap_and_the_broken_flat() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "floor_flat": "GONE"}),
        )
        .await;
        let report = call(&client, "auto_texture", json!({})).await;
        assert_eq!(report["mode"], json!("fill_missing"), "the default mode");
        assert_eq!(report["map"], json!("MAP01"), "the fixture's only map");
        assert_eq!(
            report["changed_sidedefs"],
            json!(1_u64),
            "sidedef 3's lower"
        );
        assert_eq!(report["changed_sectors"], json!(1_u64), "the unknown floor");
        assert_eq!(
            report["copied_to_working"],
            json!(false),
            "edit_sector already brought the block in"
        );
        assert_eq!(report["nodes_invalidated"], json!(false), "a texture edit");
        let sides = working_sidedefs(&client, "MAP01").await;
        assert_eq!(
            sides[3]["lower"],
            json!("STEP1"),
            "the 32-unit gap takes the wall (STEP1 is the default wall)"
        );
        let again = call(&client, "auto_texture", json!({})).await;
        assert_eq!(again["changed_sidedefs"], json!(0_u64), "idempotent");
        assert_eq!(again["changed_sectors"], json!(0_u64), "idempotent");
    }

    #[tokio::test]
    async fn auto_texture_retexture_repaints_with_the_added_wall() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        build(&client, "STARTAN3").await;
        let report = call(&client, "auto_texture", json!({"mode": "retexture"})).await;
        assert_eq!(report["mode"], json!("retexture"));
        assert_eq!(
            report["changed_sidedefs"],
            json!(7_u64),
            "six one-sided middles plus the front lower band"
        );
        assert_eq!(
            report["changed_sectors"],
            json!(0_u64),
            "the theme flats already are"
        );
        let sides = working_sidedefs(&client, "MAP01").await;
        assert_eq!(sides[0]["middle"], json!("STARTAN3"), "the hall wall");
        assert_eq!(
            sides[2]["lower"],
            json!("STARTAN3"),
            "the band repaints to wall"
        );
        assert_eq!(sides[3]["lower"], json!("-"), "retexture never fills");
        let sector0 = call(
            &client,
            "auto_texture",
            json!({"mode": "retexture", "sectors": [0_usize]}),
        )
        .await;
        assert_eq!(sector0["changed_sidedefs"], json!(0_u64), "already themed");
    }

    #[tokio::test]
    async fn auto_texture_refuses_bad_modes_and_missing_sectors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "auto_texture",
            json!({"mode": "repaint"}),
            "invalid_argument",
        )
        .await;
        refuse(
            &client,
            "auto_texture",
            json!({"sectors": [40_usize]}),
            "not_found",
        )
        .await;
        refuse(
            &client,
            "auto_texture",
            json!({"sectors": []}),
            "invalid_argument",
        )
        .await;
        let bare = connect().await;
        let (code, message, _) = call_err(&bare, "auto_texture", json!({})).await;
        assert_eq!(code, "invalid_argument", "no map anywhere");
        assert!(
            message.contains("set_active_map"),
            "the error names the fix: {message}"
        );
    }

    /// The merged view's sidedef rows for `map`, in order.
    async fn merged_sidedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "merged", "include": ["sidedefs"]}),
        )
        .await;
        report["sidedefs"]
            .as_array()
            .expect("sidedefs is an array")
            .clone()
    }

    #[tokio::test]
    async fn auto_texture_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "auto_texture", json!({})).await;
        let session = call(&client, "get_session_info", Value::Null).await;
        let labels: Vec<&str> = session["undo"]
            .as_array()
            .expect("undo list")
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .expect("labels are strings");
        assert_eq!(
            labels.first().copied().expect("one edit"),
            "auto_texture MAP01"
        );
        call(&client, "undo", Value::Null).await;
        let merged = merged_sidedefs(&client, "MAP01").await;
        assert_eq!(merged[3]["lower"], json!("-"), "the gap is open again");
    }

    #[tokio::test]
    async fn align_textures_runs_the_fixture_hall_as_one_chain() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "edit_sidedef",
            json!({"index": [0_i64, 1_i64, 4_i64], "x_offset": 7_i64}),
        )
        .await;
        let report = call(&client, "align_textures", json!({})).await;
        assert_eq!(report["aligned"], json!(3_u64), "the three bent sides");
        assert_eq!(report["warnings"], json!([]), "WALL1 has a width");
        assert_eq!(report["map"], json!("MAP01"));
        let sides = working_sidedefs(&client, "MAP01").await;
        assert_eq!(
            sides[0]["x_offset"],
            json!(0_i64),
            "the chain restarts at 0"
        );
        assert_eq!(
            sides[1]["x_offset"],
            json!(0_i64),
            "256 into a 64-wide tile"
        );
        let again = call(&client, "align_textures", json!({})).await;
        assert_eq!(again["aligned"], json!(0_u64), "idempotent");
    }

    #[tokio::test]
    async fn align_textures_selectors_and_refusals() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "edit_sidedef",
            json!({"index": 5_i64, "y_offset": 9_i64}),
        )
        .await;
        let report = call(
            &client,
            "align_textures",
            json!({"sector": 1_usize, "x": false, "y": true}),
        )
        .await;
        assert_eq!(
            report["aligned"],
            json!(1_u64),
            "sidedef 5 pegs back to 128"
        );
        let sides = working_sidedefs(&client, "MAP01").await;
        assert_eq!(sides[5]["y_offset"], json!(0_i64));
        refuse(
            &client,
            "align_textures",
            json!({"linedefs": [0_usize], "sector": 1_usize}),
            "invalid_argument",
        )
        .await;
        refuse(
            &client,
            "align_textures",
            json!({"x": false, "y": false}),
            "invalid_argument",
        )
        .await;
        refuse(
            &client,
            "align_textures",
            json!({"linedefs": [99_usize]}),
            "not_found",
        )
        .await;
    }
}
