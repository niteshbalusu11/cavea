// Settings (09 §7, CON-012): the workspace's name, its members (change a
// role, remove, invite with a link shown once, pending invitations with
// revoke) and the audit log. Only an owner makes or unmakes an owner, and
// a workspace keeps one owner: what the server would refuse is not offered,
// and its refusal is shown when it comes anyway (a role changed elsewhere).
import { useSearchParams } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
  checkAuditList,
  checkInvitationCreated,
  checkInvitationList,
  checkMember,
  checkMemberList,
  checkWorkspace,
} from "../api/check.ts";
import { call } from "../api/client.ts";
import type {
  AuditEntry,
  Invitation,
  InvitationCreated,
  WorkspaceMember,
} from "../api/generated.ts";
import { createPages, loadAll } from "../api/pages.ts";
import type { Role } from "../api/permissions.ts";
import { asApiError, componentSignal, createQuery } from "../api/query.ts";
import { msg, msgOf } from "../i18n/messages.ts";
import { adoptWorkspaceName, loadSession, me, workspace } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Field, fieldInputClass, FormError, length } from "../ui/form.tsx";
import { Confirm, Dialog, notify } from "../ui/overlays.tsx";
import { describe, reference, report } from "./errors.ts";
import {
  Action,
  CopyField,
  Empty,
  formatDateTime,
  LoadMore,
  Paged,
  SelectField,
  Tabs,
  table,
  tableBox,
  td,
  th,
} from "./kit.tsx";
import {
  inviteRefusal,
  memberLock,
  memberRefusal,
  roleChoices,
  type InviteField,
} from "./refusals.ts";

type Tab = "workspace" | "members" | "audit";
const TABS: Tab[] = ["workspace", "members", "audit"];

const permissions = () => workspace()?.permissions ?? [];
const roleLabel = (r: Role) => msg(`role.${r}`);
const isMe = (m: WorkspaceMember) => me()?.user.id === m.user.id;
const inviteChoices = () => roleChoices(permissions());

export function Settings() {
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const tab = (): Tab => TABS.find((t) => t === params.tab) ?? "workspace";
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("settings.title")}</h1>
      <Tabs
        label={msg("settings.title")}
        value={tab()}
        onChange={(t) => setParams({ tab: t === "workspace" ? undefined : t }, { replace: true })}
        tabs={[
          { value: "workspace", label: msg("settings.workspace"), content: () => <WorkspaceTab /> },
          { value: "members", label: msg("settings.members"), content: () => <MembersTab /> },
          { value: "audit", label: msg("settings.audit"), content: () => <AuditTab /> },
        ]}
      />
    </>
  );
}

function WorkspaceTab() {
  const signal = componentSignal();
  const [name, setName] = createSignal(workspace()?.name ?? "");
  const [error, setError] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const check = () => {
    const v = name().trim();
    const problem = v === "" || length(v) > 128 ? msg("charts.textRule") : null;
    setError(problem);
    return problem === null;
  };
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const ws = workspace();
    if (!check() || busy() || ws === null) return;
    setBusy(true);
    setFailure(null);
    try {
      const answer = checkWorkspace(
        await call("renameWorkspace", { body: { name: name().trim() } }, signal),
      );
      adoptWorkspaceName(answer.id, answer.name);
      notify(msg("workspace.renamed", { name: answer.name }));
    } catch (err) {
      const refusal = asApiError(err);
      if (signal.aborted || refusal.sessionLost) return;
      if (refusal.reason === "invalid_field") {
        setError(msg("charts.textRule"));
      } else {
        if (refusal.code === "forbidden") void loadSession();
        const detail = reference(refusal);
        setFailure(
          detail === undefined ? { text: describe(refusal) } : { text: describe(refusal), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="max-w-md space-y-4" noValidate onSubmit={(e) => void submit(e)}>
      <Field
        label={msg("workspace.name")}
        value={name()}
        maxLength={128}
        onInput={(v) => {
          setName(v);
          if (error() !== null) check();
        }}
        onBlur={check}
        error={error()}
      />
      <div class="space-y-1">
        <p class="text-sm font-medium text-fg">{msg("workspace.key")}</p>
        <p class="font-mono text-base text-muted">{workspace()?.key}</p>
        <p class="text-sm text-muted">{msg("workspace.keyHint")}</p>
      </div>
      <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
      <Button
        type="submit"
        variant="primary"
        class="h-8"
        disabled={busy() || name().trim() === workspace()?.name}
      >
        {busy() ? msg("workspace.saving") : msg("workspace.save")}
      </Button>
    </form>
  );
}

const pageQuery = (after: string | undefined, limit: number) =>
  after === undefined ? { limit } : { limit, after };

type Pending =
  | { kind: "role"; member: WorkspaceMember; to: Role }
  | { kind: "remove"; member: WorkspaceMember };

function MembersTab() {
  const signal = componentSignal();
  const members = createPages(
    () => workspace()?.id ?? null,
    async (_ws, after, s) =>
      checkMemberList(await call("listMembers", { query: pageQuery(after, 100) }, s)),
  );
  const invitations = createPages(
    () => workspace()?.id ?? null,
    async (_ws, after, s) =>
      checkInvitationList(await call("listInvitations", { query: pageQuery(after, 100) }, s)),
  );
  const [inviting, setInviting] = createSignal(false);
  const [pending, setPending] = createSignal<Pending | null>(null);
  const [revoking, setRevoking] = createSignal<Invitation | null>(null);

  /** The number of owners when every member is loaded. */
  const owners = createMemo(() => {
    const s = members.state();
    if (s.status !== "ready" || s.data.next !== null) return null;
    return s.data.data.filter((m) => m.role === "owner").length;
  });

  /** A refusal: said in a toast; a member gone, or a role that may have
   * changed under the person, reloads. */
  const refused = (e: unknown) => {
    const error = asApiError(e);
    const known = memberRefusal(error);
    if (known === null) return report(e);
    notify(known.message, "error", reference(error));
    members.refetch();
    if (error.code === "forbidden") void loadSession();
  };

  const apply = async (p: Pending) => {
    try {
      if (p.kind === "role") {
        const answer = checkMember(
          await call(
            "changeMemberRole",
            { path: { user: p.member.user.id }, body: { role: p.to } },
            signal,
          ),
        );
        members.update((rows) => rows.map((r) => (r.user.id === answer.user.id ? answer : r)));
        notify(msg("members.changed", { email: p.member.user.email, role: roleLabel(p.to) }));
      } else {
        await call("removeMember", { path: { user: p.member.user.id } }, signal);
        members.update((rows) => rows.filter((r) => r.user.id !== p.member.user.id));
        notify(msg("members.removed", { email: p.member.user.email }));
      }
      // One's own role or membership: the console follows it.
      if (isMe(p.member)) void loadSession();
    } catch (e) {
      refused(e);
    }
  };

  const revoke = async (inv: Invitation) => {
    try {
      await call("revokeInvitation", { path: { invitation: inv.id } }, signal);
      // Read the loaded pages again rather than drop the row: a revoked
      // invitation is deleted, and the next page's cursor may name it.
      invitations.refetch();
      notify(msg("invite.revoked", { email: inv.email }));
    } catch (e) {
      const error = asApiError(e);
      if (error.code === "not_found") {
        invitations.refetch();
        notify(msg("invite.alreadyGone"), "error", reference(error));
      } else {
        report(e);
      }
    }
  };

  const confirmText = () => {
    const p = pending();
    if (p === null) return { title: "", body: "", confirm: "" };
    const email = p.member.user.email;
    const self = isMe(p.member);
    if (p.kind === "remove") {
      return {
        title: msg("members.removeTitle", { email }),
        body: self ? msg("members.removeSelfBody") : msg("members.removeBody"),
        confirm: msg("members.removeConfirm"),
      };
    }
    const owner = p.to === "owner" || p.member.role === "owner";
    return {
      title: msg("members.changeTitle", {
        email,
        from: roleLabel(p.member.role),
        to: roleLabel(p.to),
      }),
      body: self
        ? msg("members.changeSelfBody")
        : owner
          ? msg("members.changeOwnerBody")
          : msg("members.changeBody"),
      confirm: msg("members.changeConfirm"),
    };
  };

  return (
    <div class="space-y-8">
      <section>
        <div class="flex flex-wrap items-center gap-3">
          <h2 class="mr-auto text-lg font-semibold text-fg">{msg("members.title")}</h2>
          <Action
            label={msg("invite.new")}
            icon="plus"
            variant="primary"
            blocked={null}
            onClick={() => setInviting(true)}
          />
        </div>
        <Paged pages={members} loading={msg("members.loading")} failed={msg("members.failed")}>
          {(rows) => (
            <>
              <div class={`mt-3 ${tableBox}`}>
                <table class={table}>
                  <caption class="sr-only">{msg("members.title")}</caption>
                  <thead>
                    <tr>
                      <th scope="col" class={th}>
                        {msg("members.colEmail")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("members.colRole")}
                      </th>
                      <th scope="col" class={th}>
                        <span class="sr-only">{msg("events.colActions")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={rows()}>
                      {(m) => (
                        <MemberRow
                          member={m}
                          self={isMe(m)}
                          owners={owners()}
                          onRole={(to) => setPending({ kind: "role", member: m, to })}
                          onRemove={() => setPending({ kind: "remove", member: m })}
                        />
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
              <LoadMore pages={members} />
            </>
          )}
        </Paged>
      </section>

      <section>
        <h2 class="text-lg font-semibold text-fg">{msg("invite.pendingTitle")}</h2>
        <p class="mt-1 text-base text-muted">{msg("invite.pendingBody")}</p>
        <Paged pages={invitations} loading={msg("invite.loading")} failed={msg("invite.failed")}>
          {(rows) => (
            <Show
              when={rows().length > 0}
              fallback={<p class="mt-3 text-base text-muted">{msg("invite.none")}</p>}
            >
              <div class={`mt-3 ${tableBox}`}>
                <table class={table}>
                  <caption class="sr-only">{msg("invite.pendingTitle")}</caption>
                  <thead>
                    <tr>
                      <th scope="col" class={th}>
                        {msg("members.colEmail")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("members.colRole")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("invite.colCreated")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("invite.colExpires")}
                      </th>
                      <th scope="col" class={th}>
                        <span class="sr-only">{msg("events.colActions")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={rows()}>
                      {(inv) => (
                        <tr>
                          <td class={td}>{inv.email}</td>
                          <td class={td}>{roleLabel(inv.role)}</td>
                          <td class={`${td} text-sm text-muted`}>
                            {formatDateTime(inv.created_at)}
                          </td>
                          <td class={`${td} text-sm text-muted`}>
                            {formatDateTime(inv.expires_at)}
                          </td>
                          <td class={`${td} text-right`}>
                            <Action
                              label={msg("invite.revoke")}
                              class="h-7"
                              variant="danger"
                              blocked={null}
                              onClick={() => setRevoking(inv)}
                            />
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
              <LoadMore pages={invitations} />
            </Show>
          )}
        </Paged>
      </section>

      <Show when={inviting()}>
        <InviteDialog onClose={() => setInviting(false)} onCreated={() => invitations.refetch()} />
      </Show>
      <Confirm
        open={pending() !== null}
        title={confirmText().title}
        description={confirmText().body}
        confirm={confirmText().confirm}
        variant="danger"
        onConfirm={() => {
          const p = pending();
          if (p !== null) void apply(p);
        }}
        onClose={() => setPending(null)}
      />
      <Confirm
        open={revoking() !== null}
        title={msg("invite.revokeTitle", { email: revoking()?.email ?? "" })}
        description={msg("invite.revokeBody")}
        confirm={msg("invite.revokeConfirm")}
        variant="danger"
        onConfirm={() => {
          const inv = revoking();
          if (inv !== null) void revoke(inv);
        }}
        onClose={() => setRevoking(null)}
      />
    </div>
  );
}

function MemberRow(props: {
  member: WorkspaceMember;
  self: boolean;
  owners: number | null;
  onRole: (to: Role) => void;
  onRemove: () => void;
}) {
  const lock = () => memberLock(permissions(), props.member.role, props.owners);
  const reason = () => {
    const l = lock();
    return l === "ownership"
      ? msg("members.ownerOnly")
      : l === "lastOwner"
        ? msg("members.lastOwner")
        : null;
  };
  const choices = () => roleChoices(permissions(), props.member.role);
  return (
    <tr>
      <td class={td}>
        {props.member.user.email}
        <Show when={props.self}>
          <span class="ml-2 text-sm text-muted">{msg("members.you")}</span>
        </Show>
      </td>
      <td class={td}>
        <Show
          when={reason() === null}
          fallback={
            <span>
              {roleLabel(props.member.role)}
              <span class="block text-sm text-muted">{reason()}</span>
            </span>
          }
        >
          <select
            class={`${fieldInputClass} max-w-48`}
            aria-label={msg("members.roleOf", { email: props.member.user.email })}
            value={props.member.role}
            onChange={(e) => {
              const to = choices().find((r) => r === e.currentTarget.value);
              // The list shows the role the server holds until it agrees.
              e.currentTarget.value = props.member.role;
              if (to !== undefined && to !== props.member.role) props.onRole(to);
            }}
          >
            <For each={choices()}>{(r) => <option value={r}>{roleLabel(r)}</option>}</For>
          </select>
        </Show>
      </td>
      <td class={`${td} text-right`}>
        <Action
          label={msg("members.remove")}
          class="h-7"
          variant="danger"
          blocked={reason()}
          onClick={props.onRemove}
        />
      </td>
    </tr>
  );
}

type InviteErrors = Partial<Record<InviteField, string | null>>;

/** The link as a whole URL: the server gives a path alone when it has no
 * `CAVEA_PUBLIC_URL`, and the console's origin is the server's. */
export const absolute = (link: string): string =>
  link.startsWith("/") ? `${window.location.origin}${link}` : link;

/** Invites an email with a role, then shows the link once. */
function InviteDialog(props: { onClose: () => void; onCreated: () => void }) {
  const signal = componentSignal();
  const [email, setEmail] = createSignal("");
  const [role, setRole] = createSignal<Role>("editor");
  const [errors, setErrors] = createSignal<InviteErrors>({});
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  // The link: held only while the dialog is open.
  const [created, setCreated] = createSignal<InvitationCreated | null>(null);

  const checkEmail = () => {
    const v = email().trim();
    const problem = v === "" || !v.includes("@") ? msg("invite.emailInvalid") : null;
    setErrors((e) => ({ ...e, email: problem }));
    return problem === null;
  };
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (!checkEmail() || busy()) return;
    setBusy(true);
    setFailure(null);
    try {
      setCreated(
        checkInvitationCreated(
          await call("inviteMember", { body: { email: email().trim(), role: role() } }, signal),
        ),
      );
      props.onCreated();
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const refusal = inviteRefusal(error);
      if (refusal !== null) {
        setErrors((x) => ({ ...x, [refusal.field]: refusal.message }));
        if (error.code === "forbidden") void loadSession();
      } else {
        if (error.code === "forbidden") void loadSession();
        const detail = reference(error);
        setFailure(
          detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };
  const close = () => {
    setCreated(null);
    props.onClose();
  };
  return (
    <Show
      when={created()}
      fallback={
        <Dialog open title={msg("invite.title")} heading onClose={close}>
          <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
            <p class="text-base text-muted">{msg("invite.body")}</p>
            <Field
              label={msg("invite.email")}
              type="email"
              autocomplete="off"
              value={email()}
              onInput={(v) => {
                setEmail(v);
                if (errors().email) checkEmail();
              }}
              onBlur={checkEmail}
              error={errors().email ?? null}
            />
            <SelectField
              label={msg("invite.role")}
              value={role()}
              options={inviteChoices().map((r) => ({ value: r, label: roleLabel(r) }))}
              onChange={(r) => {
                setRole(r);
                setErrors((x) => ({ ...x, role: null }));
              }}
              hint={msg(`invite.roleHint.${role()}`)}
              error={errors().role ?? null}
            />
            <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
            <div class="flex justify-end gap-2">
              <Button class="h-8" onClick={close}>
                {msg("action.cancel")}
              </Button>
              <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
                {busy() ? msg("invite.creating") : msg("invite.create")}
              </Button>
            </div>
          </form>
        </Dialog>
      }
    >
      {(c) => (
        <Confirm
          open
          title={msg("invite.shownTitle", { email: c().invitation.email })}
          description={msg("invite.shownBody", {
            role: roleLabel(c().invitation.role),
            expires: formatDateTime(c().invitation.expires_at),
          })}
          onClose={close}
        >
          <div class="mt-4">
            <CopyField label={msg("invite.link")} value={absolute(c().link)} />
          </div>
        </Confirm>
      )}
    </Show>
  );
}

/** At most this many pages of members are read to name the log's actors. */
const MEMBER_PAGES = 10;

function AuditTab() {
  const entries = createPages(
    () => workspace()?.id ?? null,
    async (_ws, after, s) =>
      checkAuditList(await call("listAudit", { query: pageQuery(after, 50) }, s)),
  );
  // Members' emails for the identifiers the log holds; a former member
  // stays an identifier.
  const members = createQuery(
    () => workspace()?.id ?? null,
    (_ws, s) =>
      loadAll(
        async (after) =>
          checkMemberList(await call("listMembers", { query: pageQuery(after, 100) }, s)),
        MEMBER_PAGES,
      ),
  );
  const who = (id: string | null): string => {
    if (id === null) return "—";
    const s = members.state();
    const m = s.status === "ready" ? s.data.data.find((x) => x.user.id === id) : undefined;
    if (m !== undefined) return m.user.email;
    if (id === "admin") return msg("audit.admin");
    if (id === "anonymous") return msg("audit.anonymous");
    return id;
  };
  return (
    <>
      <p class="max-w-2xl text-base text-muted">{msg("audit.intro")}</p>
      <Paged pages={entries} loading={msg("audit.loading")} failed={msg("audit.failed")}>
        {(rows) => (
          <Show
            when={rows().length > 0}
            fallback={
              <Empty icon="report" title={msg("audit.emptyTitle")} body={msg("audit.emptyBody")} />
            }
          >
            <div class={`mt-3 ${tableBox}`}>
              <table class={table}>
                <caption class="sr-only">{msg("settings.audit")}</caption>
                <thead>
                  <tr>
                    <th scope="col" class={th}>
                      {msg("audit.colWhen")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("audit.colActor")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("audit.colAction")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("audit.colTarget")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("audit.colDetail")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <For each={rows()}>{(entry) => <AuditRow entry={entry} who={who} />}</For>
                </tbody>
              </table>
            </div>
            <LoadMore pages={entries} />
          </Show>
        )}
      </Paged>
    </>
  );
}

const factText = (v: unknown) =>
  typeof v === "string" || typeof v === "number" ? String(v) : null;

/** An entry's small facts as text: `role`, `from` → `to`, `kind`, `version`. */
export function auditDetail(detail: AuditEntry["detail"]): string {
  if (detail === null) return "";
  const role = (v: unknown) => {
    const t = factText(v);
    return t === null ? null : (msgOf("role", t) ?? t);
  };
  const parts: string[] = [];
  const from = role(detail["from"]);
  const to = role(detail["to"]);
  if (from !== null && to !== null) parts.push(msg("audit.fromTo", { from, to }));
  const r = role(detail["role"]);
  if (r !== null) parts.push(msg("audit.role", { role: r }));
  const kind = factText(detail["kind"]);
  if (kind !== null) parts.push(msgOf("audit.kind", kind) ?? kind);
  const version = factText(detail["version"]);
  if (version !== null) parts.push(msg("audit.version", { version }));
  return parts.join(" · ");
}

function AuditRow(props: { entry: AuditEntry; who: (id: string | null) => string }) {
  return (
    <tr>
      <td class={`${td} text-sm whitespace-nowrap text-muted`}>{formatDateTime(props.entry.at)}</td>
      <td class={`${td} text-sm`}>{props.who(props.entry.actor)}</td>
      <td class={td}>{msgOf("audit.action", props.entry.action) ?? props.entry.action}</td>
      <td class={`${td} text-sm`}>
        {props.entry.target?.startsWith("usr_") === true
          ? props.who(props.entry.target)
          : (props.entry.target ?? "—")}
      </td>
      <td class={`${td} text-sm text-muted`}>
        {auditDetail(props.entry.detail)}
        <Show when={props.entry.request_id}>
          {(id) => <span class="block font-mono text-xs text-faint">{id()}</span>}
        </Show>
      </td>
    </tr>
  );
}
