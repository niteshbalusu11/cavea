//! A workspace's members against PostgreSQL (09 §7, CON-012): role changes
//! and removals on the next request, and the owner rules over random
//! sequences. The routes' credentials and roles are the matrix's
//! (`authorisation_matrix`).

use super::*;

async fn as_member(
    app: &Router,
    method: Method,
    uri: &str,
    session: &SessionId,
    workspace: &str,
    body: Option<Value>,
) -> (Response<Body>, Value) {
    let headers = console(session, Some(workspace));
    call_with(app, method, uri, None, &refs(&headers), body).await
}

/// CON-004, CON-012: a lowered role and a removal apply to the member's very
/// next request, with the same session; the user can still sign in and use
/// `me`, which lists no workspace.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn role_changes_apply_to_the_next_request() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (_, owner) = member(&pool, &a.id, Role::Owner, "owner@a.example").await;
        let (editor, session) = member(&pool, &a.id, Role::Editor, "editor@a.example").await;
        let draft = format!("/v1/charts/{}/draft", a.chart);
        let save = async || {
            let mut headers = console(&session, Some(&a.id));
            headers.push((header::IF_MATCH, "*".to_owned()));
            let request = request(
                Method::PUT,
                &draft,
                None,
                &refs(&headers),
                Some(a.document.clone()),
            );
            send(app.clone(), request).await
        };
        let (response, body) = save().await;
        assert_eq!(response.status(), 200, "{body}");

        let uri = format!("/console/api/members/usr_{editor}");
        let body = json!({ "role": "viewer" });
        let (response, changed) =
            as_member(&app, Method::PATCH, &uri, &owner, &a.id, Some(body)).await;
        assert_eq!(response.status(), 200, "{changed}");
        assert_eq!(changed["role"], "viewer");
        let (response, refused) = save().await;
        assert_problem(&response, &refused, Code::Forbidden);
        assert_eq!(refused["details"]["permission"], "charts.write");
        let (response, _) = as_member(&app, Method::GET, &draft, &session, &a.id, None).await;
        assert_eq!(response.status(), 200);

        let (response, _) = as_member(&app, Method::DELETE, &uri, &owner, &a.id, None).await;
        assert_eq!(response.status(), 204);
        let (response, refused) = as_member(&app, Method::GET, &draft, &session, &a.id, None).await;
        assert_problem(&response, &refused, Code::Forbidden);
        assert_eq!(refused["details"]["reason"], "workspace");
        let headers = console(&session, None);
        let (response, me) = call_with(
            &app,
            Method::GET,
            "/console/api/me",
            None,
            &refs(&headers),
            None,
        )
        .await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["workspaces"], json!([]));
        let body = json!({ "email": "editor@a.example", "password": PASSWORD });
        let (response, me) =
            call(&app, Method::POST, "/console/api/session", None, Some(body)).await;
        assert_eq!(response.status(), 200, "{me}");
    })
    .await;
}

/// 09 §2, §7: over random sequences of role changes, removals and returns
/// by random members, every answer is the model's (the role table, `404`
/// for a non-member, `ownership` to make or unmake an owner, `last_owner`),
/// the database agrees with the model, and every step leaves an owner.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn a_workspace_always_keeps_an_owner() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let start = [
            Role::Owner,
            Role::Admin,
            Role::Admin,
            Role::Editor,
            Role::Viewer,
        ];
        let mut people = Vec::new();
        for (i, role) in start.into_iter().enumerate() {
            let (user, session) = member(&pool, &a.id, role, &format!("p{i}@a.example")).await;
            people.push((user, session, Some(role)));
        }
        let mut seed: u64 = 0x5eed_cafe;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % n as u64).unwrap()
        };
        let mut successes = 0;
        for step in 0..160 {
            let actor = next(people.len());
            let target = next(people.len());
            let op = next(7);
            // Someone removed comes back (as invitations will make them).
            if op == 6 {
                if people[target].2.is_none() {
                    sqlx::query(
                        "INSERT INTO memberships (user_id, workspace_id, role)
                         VALUES ($1::uuid, $2::uuid, 'viewer')",
                    )
                    .bind(&people[target].0)
                    .bind(&a.id[3..])
                    .execute(&pool)
                    .await
                    .unwrap();
                    people[target].2 = Some(Role::Viewer);
                }
                continue;
            }
            let new = (op < 5).then(|| Role::ALL[op]);
            let owners = people.iter().filter(|p| p.2 == Some(Role::Owner)).count();
            let (acting, old) = (people[actor].2, people[target].2);
            let expected: Result<(), &str> = match (acting, old) {
                (None, _) => Err("workspace"),
                (Some(r), _) if !r.allows(Permission::Settings) => Err("role"),
                (_, None) => Err("member"),
                (Some(r), Some(old))
                    if (old == Role::Owner || new == Some(Role::Owner))
                        && !r.allows(Permission::Ownership) =>
                {
                    let _ = old;
                    Err("role")
                }
                (_, Some(Role::Owner)) if new != Some(Role::Owner) && owners == 1 => {
                    Err("last_owner")
                }
                _ => Ok(()),
            };
            let uri = format!("/console/api/members/usr_{}", people[target].0);
            let (method, body) = match new {
                Some(role) => (Method::PATCH, Some(json!({ "role": role.as_str() }))),
                None => (Method::DELETE, None),
            };
            let (response, answer) =
                as_member(&app, method, &uri, &people[actor].1, &a.id, body).await;
            let at = format!("step {step}: {actor} {new:?} {target}: {answer}");
            match expected {
                Ok(()) => {
                    assert!(response.status().is_success(), "{at}");
                    people[target].2 = new;
                    successes += 1;
                }
                Err("member") => assert_eq!(response.status(), 404, "{at}"),
                Err(reason) => assert_eq!(answer["details"]["reason"], reason, "{at}"),
            }
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT replace(user_id::text, '-', ''), role FROM memberships
                  WHERE workspace_id = $1::uuid",
            )
            .bind(&a.id[3..])
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(rows.iter().any(|(_, r)| r == "owner"), "{at}");
            for (user, _, role) in &people {
                let stored = rows
                    .iter()
                    .find(|(u, _)| u == user)
                    .map(|(_, r)| r.as_str());
                assert_eq!(stored, role.map(Role::as_str), "{at}");
            }
        }
        assert!(successes > 20, "{successes}");
    })
    .await;
}
