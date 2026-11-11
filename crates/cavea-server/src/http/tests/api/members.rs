//! A workspace's people against PostgreSQL (09 §7, CON-012, CON-013): role
//! changes and removals on the next request, the owner rules over random
//! sequences, invitations, and the audit log's rows and transactions. The
//! routes' credentials and roles are the matrix's (`authorisation_matrix`).

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
            // Someone removed comes back, as by an invitation.
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

fn token_of(link: &str) -> &str {
    link.split_once("#token=").unwrap().1
}

/// CON-012: an invitation's link works once, before it expires, for its own
/// email: a new user sets a password and is signed in; an existing one must
/// be signed in as that email, with the session's CSRF token; another
/// account's session is refused and leaves the invitation. An admin cannot
/// invite an owner; the admin key can. No token, link or email is logged.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn invitations_work_once_for_their_email() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let (_, owner) = member(&pool, &a.id, Role::Owner, "owner@a.example").await;
        let (_, admin) = member(&pool, &a.id, Role::Admin, "admin@a.example").await;
        let (_, elsewhere) = member(&pool, &b.id, Role::Viewer, "known@b.example").await;
        let (_, other) = member(&pool, &b.id, Role::Viewer, "other@b.example").await;
        let invite = async |session: &SessionId, email: &str, role: &str| {
            let body = json!({ "email": email, "role": role });
            as_member(
                &app,
                Method::POST,
                "/console/api/members",
                session,
                &a.id,
                Some(body),
            )
            .await
        };
        let accept = async |session: Option<&SessionId>, body: Value| {
            let headers = session.map(|s| console(s, None)).unwrap_or_default();
            let uri = "/console/api/invitations/accept";
            call_with(&app, Method::POST, uri, None, &refs(&headers), Some(body)).await
        };
        let mut links = Vec::new();

        let (response, refused) = invite(&admin, "boss@a.example", "owner").await;
        assert_problem(&response, &refused, Code::Forbidden);
        assert_eq!(refused["details"]["permission"], "ownership");
        let (response, created) = invite(&admin, "New@A.example", "editor").await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(created["invitation"]["email"], "new@a.example");
        let link = created["link"].as_str().unwrap().to_owned();
        assert!(link.starts_with("/console/invite#token="), "{link}");
        let token = token_of(&link).to_owned();
        let (response, joined) =
            accept(None, json!({ "token": token, "password": PASSWORD })).await;
        assert_eq!(response.status(), 200, "{joined}");
        assert_eq!(joined["workspaces"][0]["role"], "editor");
        assert!(response.headers().contains_key(header::SET_COOKIE));
        // Single use.
        let body = json!({ "token": token, "password": PASSWORD });
        let (response, replay) = accept(None, body).await;
        assert_problem(&response, &replay, Code::Unauthorized);
        assert_eq!(replay["details"]["reason"], "invitation");
        links.push(link);

        // Expired.
        let (_, created) = invite(&owner, "late@a.example", "viewer").await;
        let link = created["link"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE invitations SET expires_at = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let body = json!({ "token": token_of(&link), "password": PASSWORD });
        let (response, late) = accept(None, body).await;
        assert_eq!(late["details"]["reason"], "invitation", "{late}");
        assert_eq!(response.status(), 401);
        links.push(link);

        // An email with an account: not signed in, then another account's
        // session, then without the CSRF token, then its own.
        let (_, created) = invite(&owner, "known@b.example", "operator").await;
        let link = created["link"].as_str().unwrap().to_owned();
        let token = token_of(&link).to_owned();
        let body = json!({ "token": token, "password": PASSWORD });
        let (response, problem) = accept(None, body).await;
        assert_eq!(response.status(), 409, "{problem}");
        assert_eq!(problem["details"]["reason"], "sign_in");
        let (response, problem) = accept(Some(&other), json!({ "token": token })).await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "invitation_email");
        let cookie = console(&elsewhere, None);
        let no_csrf: Vec<_> = refs(&cookie)
            .into_iter()
            .filter(|(n, _)| *n != crate::http::staff::CSRF)
            .collect();
        let body = json!({ "token": token });
        let uri = "/console/api/invitations/accept";
        let (response, problem) =
            call_with(&app, Method::POST, uri, None, &no_csrf, Some(body)).await;
        assert_eq!(problem["details"]["reason"], "csrf", "{problem}");
        assert_eq!(response.status(), 403);
        let (response, joined) = accept(Some(&elsewhere), json!({ "token": token })).await;
        assert_eq!(response.status(), 200, "{joined}");
        let roles: Vec<&str> = joined["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["role"].as_str().unwrap())
            .collect();
        // In the order the workspaces were made: A, then B.
        assert_eq!(roles, ["operator", "viewer"]);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        links.push(link);

        // Pending invitations list; a revoked one stops working.
        let (_, created) = invite(&owner, "maybe@a.example", "viewer").await;
        let link = created["link"].as_str().unwrap().to_owned();
        let (_, listed) = as_member(
            &app,
            Method::GET,
            "/console/api/invitations",
            &owner,
            &a.id,
            None,
        )
        .await;
        assert_eq!(listed["data"].as_array().unwrap().len(), 1, "{listed}");
        assert!(!listed.to_string().contains(token_of(&link)));
        let uri = format!(
            "/console/api/invitations/{}",
            created["invitation"]["id"].as_str().unwrap()
        );
        let (response, _) = as_member(&app, Method::DELETE, &uri, &owner, &a.id, None).await;
        assert_eq!(response.status(), 204);
        let body = json!({ "token": token_of(&link), "password": PASSWORD });
        let (response, _) = accept(None, body).await;
        assert_eq!(response.status(), 401);
        links.push(link);

        // The admin key's owner invitation to a workspace without members.
        let uri = format!("/v1/workspaces/{}/invitations", b.id);
        let body = json!({ "email": "founder@b.example" });
        let (response, created) = call(&app, Method::POST, &uri, Some(ADMIN), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        let link = created["link"].as_str().unwrap().to_owned();
        let body = json!({ "token": token_of(&link), "password": PASSWORD });
        let (response, joined) = accept(None, body).await;
        assert_eq!(response.status(), 200, "{joined}");
        assert_eq!(joined["workspaces"][0]["role"], "owner");
        links.push(link);

        let mut secrets: Vec<&str> = links.iter().map(|l| token_of(l)).collect();
        secrets.extend(["@a.example", "@b.example", PASSWORD]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

/// CON-013: one row per sensitive action, written in the action's own
/// transaction (a failing audit write leaves the action undone), with
/// identifiers only: no password, token, key or email, nor anything of an
/// email that is no account's.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn audit_rows_follow_their_actions() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        sqlx::query("DELETE FROM audit_log")
            .execute(&pool)
            .await
            .unwrap();
        let (owner_id, owner) = member(&pool, &a.id, Role::Owner, "owner@a.example").await;
        let sign_in = async |email: &str, password: &str| {
            let body = json!({ "email": email, "password": password });
            call(&app, Method::POST, "/console/api/session", None, Some(body)).await
        };
        let (response, _) = sign_in("nobody@a.example", "not-a-real-password-9").await;
        assert_eq!(response.status(), 401);
        let (response, _) = sign_in("owner@a.example", "not-a-real-password-9").await;
        assert_eq!(response.status(), 401);
        let (response, _) = sign_in("owner@a.example", PASSWORD).await;
        assert_eq!(response.status(), 200);
        let body = json!({ "kind": "secret", "name": "console" });
        let (response, key) =
            as_member(&app, Method::POST, "/v1/keys", &owner, &a.id, Some(body)).await;
        assert_eq!(response.status(), 201);
        let uri = format!("/v1/keys/{}", key["id"].as_str().unwrap());
        for _ in 0..2 {
            let (response, _) = call(&app, Method::DELETE, &uri, Some(&a.secret), None).await;
            assert_eq!(response.status(), 200);
        }
        let uri = format!("/v1/charts/{}/publish", a.chart);
        let (response, _) = as_member(&app, Method::POST, &uri, &owner, &a.id, None).await;
        assert_eq!(response.status(), 200);
        let body = json!({ "key": "audited", "chart": a.chart });
        let (response, _) =
            as_member(&app, Method::POST, "/v1/events", &owner, &a.id, Some(body)).await;
        assert_eq!(response.status(), 201);
        // Seat commands are the event log's: nothing here.
        let uri = format!("/v1/events/{}/change-status", a.event);
        let body = json!({ "objects": { "version": a.hash, "idx": [1] }, "status": "blocked" });
        let (response, _) = as_member(&app, Method::POST, &uri, &owner, &a.id, Some(body)).await;
        assert_eq!(response.status(), 200);
        for _ in 0..2 {
            let (response, _) = as_member(
                &app,
                Method::DELETE,
                "/v1/events/audited",
                &owner,
                &a.id,
                None,
            )
            .await;
            assert_eq!(response.status(), 200);
        }
        // A failing audit write undoes its action.
        for statement in [
            "CREATE FUNCTION refuse() RETURNS trigger LANGUAGE plpgsql AS
             $$ BEGIN RAISE EXCEPTION 'refused'; END $$",
            "CREATE TRIGGER refuse BEFORE INSERT ON audit_log FOR EACH ROW
             WHEN (NEW.action = 'chart.archived') EXECUTE FUNCTION refuse()",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        let uri = format!("/v1/charts/{}", a.chart);
        let (response, _) = as_member(&app, Method::DELETE, &uri, &owner, &a.id, None).await;
        assert_eq!(response.status(), 500);
        let (_, chart) = call(&app, Method::GET, &uri, Some(&a.secret), None).await;
        assert_eq!(chart["archived_at"], Value::Null);
        let headers = console(&owner, None);
        let (response, _) = call_with(
            &app,
            Method::DELETE,
            "/console/api/session",
            None,
            &refs(&headers),
            None,
        )
        .await;
        assert_eq!(response.status(), 204);

        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT action, actor, replace(user_id::text, '-', ''),
                        replace(workspace_id::text, '-', ''), target, detail::text
                   FROM audit_log ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            actions,
            [
                "console.sign_in_failed",
                "console.sign_in_failed",
                "console.sign_in",
                "api_key.created",
                "api_key.revoked",
                "chart.published",
                "event.created",
                "event.archived",
                "console.sign_out",
            ]
        );
        let usr = format!("usr_{owner_id}");
        let ws = &a.id[3..];
        assert_eq!(rows[0].1, "anonymous");
        assert_eq!(rows[0].2, None, "nothing of an email that is no account's");
        assert_eq!(rows[1].2.as_deref(), Some(owner_id.as_str()));
        assert_eq!(
            (rows[2].1.as_str(), rows[2].3.as_deref()),
            (usr.as_str(), None)
        );
        assert_eq!(
            (rows[3].1.as_str(), rows[3].3.as_deref()),
            (usr.as_str(), Some(ws))
        );
        assert_eq!(rows[4].1, a.secret_id);
        assert_eq!(rows[5].4.as_deref(), Some(a.chart.as_str()));
        let text = format!("{rows:?}");
        for secret in [
            PASSWORD,
            "not-a-real-password-9",
            "@a.example",
            &a.secret,
            "sk_",
        ] {
            assert!(!text.contains(secret), "{secret:.11}… in the audit log");
        }

        // The owner reads them: the workspace's, and the member's sign-ins.
        let (_, owner) = member(&pool, &a.id, Role::Admin, "reader@a.example").await;
        let (response, page) = as_member(
            &app,
            Method::GET,
            "/console/api/audit?limit=4",
            &owner,
            &a.id,
            None,
        )
        .await;
        assert_eq!(response.status(), 200, "{page}");
        assert_eq!(page["data"][0]["action"], "console.sign_out");
        let next = page["next"].as_str().unwrap();
        let uri = format!("/console/api/audit?after={next}");
        let (_, rest) = as_member(&app, Method::GET, &uri, &owner, &a.id, None).await;
        let listed =
            page["data"].as_array().unwrap().len() + rest["data"].as_array().unwrap().len();
        // All but the failed sign-in of an email that is no account's.
        assert_eq!(listed, rows.len() - 1, "{rest}");
    })
    .await;
}
