// Settings (09 §7, CON-012): the workspace's name, its members (change a
// role, remove, invite with a link shown once, pending invitations with
// revoke) and the audit log. Only an owner makes or unmakes an owner, and
// a workspace keeps one owner: what the server would refuse is not offered,
// and its refusal is shown when it comes anyway (a role changed elsewhere).
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { useSearchParams } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
  checkAuditList,
  checkInvitationCreated,
  checkInvitationList,
  checkMember,
  checkMemberList,
  checkPasswordResetCreated,
  checkWorkspace,
} from "../api/check.ts";
import { call } from "../api/client.ts";
import type {
  AuditEntry,
  Invitation,
  InvitationCreated,
  PasswordResetCreated,
  WorkspaceMember,
} from "../api/generated.ts";
import { createPages, loadAll } from "../api/pages.ts";
import type { Role } from "../api/permissions.ts";
import { asApiError, componentSignal, createQuery } from "../api/query.ts";
import { msg, msgOf } from "../i18n/messages.ts";
import { adoptWorkspaceName, loadSession, me, workspace } from "../session.ts";
import { Button, buttonClass } from "../ui/controls.tsx";
import { Field, fieldInputClass, FormError, length } from "../ui/form.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, Confirm, Dialog, menuContent, menuItem, notify } from "../ui/overlays.tsx";
import { describe, reference, report } from "./errors.ts";
import { LicenceTab } from "./Licence.tsx";
import {
  Action,
  CheckField,
  complete,
  CopyField,
  Empty,
  formatDateTime,
  LoadMore,
  Paged,
  pageSort,
  SelectField,
  Tabs,
  table,
  tableBox,
  td,
  th,
} from "./kit.tsx";
import {
  accountHelpLock,
  accountHelpRefusal,
  inviteRefusal,
  memberLock,
  memberRefusal,
  roleChoices,
  ROLES,
  type InviteField,
} from "./refusals.ts";
import { by, rove, rowKeys } from "./tables.ts";

/** Owner first: a role's rank, for sorting. */
const rank = (r: Role) => ROLES.indexOf(r);

type Tab = "workspace" | "members" | "audit" | "licence";
const TABS: Tab[] = ["workspace", "members", "audit", "licence"];

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
          { value: "licence", label: msg("settings.licence"), content: () => <LicenceTab /> },
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
  // Neither route orders by a column: sorted in the page once every row is loaded.
  const memberSort = pageSort<WorkspaceMember, "email" | "role">({
    email: { label: "members.colEmail", kind: "text", compare: by.text((m) => m.user.email) },
    role: { label: "members.colRole", kind: "number", compare: by.number((m) => rank(m.role)) },
  });
  const inviteSort = pageSort<Invitation, "email" | "role" | "created" | "expires">({
    email: { label: "members.colEmail", kind: "text", compare: by.text((i) => i.email) },
    role: { label: "members.colRole", kind: "number", compare: by.number((i) => rank(i.role)) },
    created: { label: "invite.colCreated", kind: "time", compare: by.time((i) => i.created_at) },
    expires: { label: "invite.colExpires", kind: "time", compare: by.time((i) => i.expires_at) },
  });
  const [inviting, setInviting] = createSignal(false);
  const [pending, setPending] = createSignal<Pending | null>(null);
  const [revoking, setRevoking] = createSignal<Invitation | null>(null);
  // Sign-in help for a member (CON-003, D-90): a reset link, or resetting
  // their two-step sign-in.
  const [helping, setHelping] = createSignal<{
    kind: "link" | "twoStep";
    member: WorkspaceMember;
  } | null>(null);

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

  /** A refusal of sign-in help: said in a toast; a member gone or a role
   * that may have changed (theirs or the person's own) reads the list and
   * `me` again. */
  const helpRefused = (e: unknown) => {
    const error = asApiError(e);
    const known = accountHelpRefusal(error);
    if (known === null) return report(e);
    notify(known.message, "error", reference(error));
    if (known.gone || known.stale) members.refetch();
    if (known.stale) void loadSession();
  };

  const resetTwoStep = async (member: WorkspaceMember) => {
    try {
      await call("resetMemberSecondFactor", { path: { user: member.user.id } }, signal);
      notify(msg("resetTwoStep.done", { email: member.user.email }));
    } catch (e) {
      helpRefused(e);
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
                      {memberSort.header("email", complete(members))}
                      {memberSort.header("role", complete(members))}
                      <th scope="col" class={th}>
                        <span class="sr-only">{msg("events.colActions")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody onKeyDown={rowKeys}>
                    <For each={memberSort.rows(rows(), complete(members))}>
                      {(m, i) => (
                        <MemberRow
                          first={i() === 0}
                          member={m}
                          self={isMe(m)}
                          owners={owners()}
                          onRole={(to) => setPending({ kind: "role", member: m, to })}
                          onRemove={() => setPending({ kind: "remove", member: m })}
                          onHelp={(kind) => setHelping({ kind, member: m })}
                        />
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
              {memberSort.note(complete(members))}
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
                      {inviteSort.header("email", complete(invitations))}
                      {inviteSort.header("role", complete(invitations))}
                      {inviteSort.header("created", complete(invitations))}
                      {inviteSort.header("expires", complete(invitations))}
                      <th scope="col" class={th}>
                        <span class="sr-only">{msg("events.colActions")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody onKeyDown={rowKeys}>
                    <For each={inviteSort.rows(rows(), complete(invitations))}>
                      {(inv, i) => (
                        <tr>
                          <td
                            class={td}
                            data-row-stop
                            tabIndex={i() === 0 ? 0 : -1}
                            onFocus={(e) => rove(e.currentTarget)}
                          >
                            {inv.email}
                          </td>
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
              {inviteSort.note(complete(invitations))}
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
        open={helping()?.kind === "twoStep"}
        title={msg("resetTwoStep.title", { email: helping()?.member.user.email ?? "" })}
        description={msg("resetTwoStep.body")}
        confirm={msg("resetTwoStep.confirm")}
        variant="danger"
        onConfirm={() => {
          const h = helping();
          if (h !== null) void resetTwoStep(h.member);
        }}
        onClose={() => setHelping(null)}
      />
      <Show when={helping()?.kind === "link" ? helping()?.member : undefined}>
        {(m) => (
          <ResetLinkDialog
            member={m()}
            onClose={() => setHelping(null)}
            onRefused={(e) => {
              setHelping(null);
              helpRefused(e);
            }}
          />
        )}
      </Show>
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
  first: boolean;
  member: WorkspaceMember;
  self: boolean;
  owners: number | null;
  onRole: (to: Role) => void;
  onRemove: () => void;
  onHelp: (kind: "link" | "twoStep") => void;
}) {
  const lock = () => memberLock(permissions(), props.member.role, props.owners);
  const helpLock = () => {
    const l = accountHelpLock(permissions(), props.member.role, props.self);
    return l === "self"
      ? msg("members.ownAccount")
      : l === "ownership"
        ? msg("members.ownerAccounts")
        : null;
  };
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
      <td
        class={td}
        data-row-stop
        tabIndex={props.first ? 0 : -1}
        onFocus={(e) => rove(e.currentTarget)}
      >
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
      <td class={`${td} text-right whitespace-nowrap`}>
        <span class="inline-flex gap-2">
          <Show
            when={helpLock() === null}
            fallback={
              <Action
                label={msg("members.signInHelp")}
                class="h-7"
                blocked={helpLock()}
                onClick={() => {}}
              />
            }
          >
            <DropdownMenu gutter={4} placement="bottom-end">
              <DropdownMenu.Trigger
                data-focus-id={`member-help-${props.member.user.id}`}
                class={buttonClass("secondary", "h-7")}
                aria-label={msg("members.accountMenu", { email: props.member.user.email })}
              >
                {msg("members.signInHelp")}
                <Icon name="more" />
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content class={menuContent}>
                  <DropdownMenu.Item
                    class={menuItem}
                    onSelect={afterMenu(() => props.onHelp("link"))}
                  >
                    <Icon name="lock" />
                    {msg("members.resetLink")}
                  </DropdownMenu.Item>
                  <DropdownMenu.Item
                    class={`${menuItem} text-danger`}
                    onSelect={afterMenu(() => props.onHelp("twoStep"))}
                  >
                    <Icon name="unlock" />
                    {msg("members.resetTwoStep")}
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu>
          </Show>
          <Action
            label={msg("members.remove")}
            class="h-7"
            variant="danger"
            blocked={reason()}
            onClick={props.onRemove}
          />
        </span>
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

/** Makes a password reset link for a member (D-90), then shows it once.
 * Clearing their two-step sign-in is a separate choice, off by default:
 * a lost password is not a lost phone. */
function ResetLinkDialog(props: {
  member: WorkspaceMember;
  onClose: () => void;
  /** A refusal the members screen says (the member gone, a role). */
  onRefused: (e: unknown) => void;
}) {
  const signal = componentSignal();
  const [clear, setClear] = createSignal(false);
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  // The link: held only while the dialog is open.
  const [created, setCreated] = createSignal<PasswordResetCreated | null>(null);
  const email = () => props.member.user.email;
  const close = () => {
    setCreated(null);
    props.onClose();
  };
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (busy()) return;
    setBusy(true);
    setFailure(null);
    try {
      setCreated(
        checkPasswordResetCreated(
          await call(
            "createMemberPasswordReset",
            { path: { user: props.member.user.id }, body: { clear_second_factor: clear() } },
            signal,
          ),
        ),
      );
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      if (accountHelpRefusal(error) !== null) {
        props.onRefused(error);
        return;
      }
      const detail = reference(error);
      setFailure(
        detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show
      when={created()}
      fallback={
        <Dialog open title={msg("resetLink.title", { email: email() })} heading onClose={close}>
          <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
            <p class="text-base text-muted">{msg("resetLink.body", { email: email() })}</p>
            <CheckField
              label={msg("resetLink.clear")}
              hint={msg("resetLink.clearHint")}
              checked={clear()}
              onChange={setClear}
            />
            <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
            <div class="flex justify-end gap-2">
              <Button class="h-8" onClick={close}>
                {msg("action.cancel")}
              </Button>
              <Button type="submit" variant="primary" class="h-8" disabled={busy()}>
                {busy() ? msg("resetLink.creating") : msg("resetLink.create")}
              </Button>
            </div>
          </form>
        </Dialog>
      }
    >
      {(c) => (
        <Confirm
          open
          title={msg("resetLink.shownTitle", { email: email() })}
          description={msg("resetLink.shownBody", {
            email: email(),
            expires: formatDateTime(c().reset.expires_at),
          })}
          onClose={close}
        >
          <div class="mt-4 space-y-2">
            <CopyField label={msg("resetLink.link")} value={absolute(c().link)} />
            <Show when={c().reset.clear_second_factor}>
              <p class="text-sm text-warning">{msg("resetLink.shownCleared")}</p>
            </Show>
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
  // Newest first from the server, which orders by nothing else: sorted in
  // the page once the whole log is loaded.
  const sort = pageSort<AuditEntry, "when" | "actor" | "action">({
    when: { label: "audit.colWhen", kind: "time", compare: by.time((e) => e.at) },
    actor: { label: "audit.colActor", kind: "text", compare: by.text((e) => who(e.actor)) },
    action: { label: "audit.colAction", kind: "text", compare: by.text((e) => actionName(e)) },
  });
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
                    {sort.header("when", complete(entries))}
                    {sort.header("actor", complete(entries))}
                    {sort.header("action", complete(entries))}
                    <th scope="col" class={th}>
                      {msg("audit.colTarget")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("audit.colDetail")}
                    </th>
                  </tr>
                </thead>
                <tbody onKeyDown={rowKeys}>
                  <For each={sort.rows(rows(), complete(entries))}>
                    {(entry, i) => <AuditRow entry={entry} who={who} first={i() === 0} />}
                  </For>
                </tbody>
              </table>
            </div>
            {sort.note(complete(entries))}
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
  // Account security (CON-003): how a sign-in was completed, codes left,
  // a reset that removed two-step sign-in.
  const factor = detail["second_factor"];
  if (factor === "totp") parts.push(msg("audit.byApp"));
  if (factor === "recovery_code") parts.push(msg("audit.byRecovery"));
  const remaining = detail["remaining"];
  if (typeof remaining === "number") parts.push(msg("audit.remaining", { n: remaining }));
  if (detail["second_factor_cleared"] === true || detail["clear_second_factor"] === true) {
    parts.push(msg("audit.cleared"));
  }
  return parts.join(" · ");
}

const actionName = (e: AuditEntry) => msgOf("audit.action", e.action) ?? e.action;

function AuditRow(props: {
  entry: AuditEntry;
  who: (id: string | null) => string;
  first: boolean;
}) {
  return (
    <tr>
      <td
        class={`${td} text-sm whitespace-nowrap text-muted`}
        data-row-stop
        tabIndex={props.first ? 0 : -1}
        onFocus={(e) => rove(e.currentTarget)}
      >
        {formatDateTime(props.entry.at)}
      </td>
      <td class={`${td} text-sm`}>{props.who(props.entry.actor)}</td>
      <td class={td}>{actionName(props.entry)}</td>
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
