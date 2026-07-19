import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";
import { activeSubscriptionGroupIds, buildAccountAlerts, groupSupportsTool, isActiveSubscription, money, moneyOrDash, percentLabel, prioritizeByActiveSubscription, quotaLine, recommendedSubscriptionGroupId, subscriptionProgressInfo, subscriptionsWithResolvedGroups, usageWindow, type GroupSummary, type SubscriptionProgress, type SubscriptionProgressInfo, type SubscriptionSummary } from "./subscription";
import CodexOfficialAccount from "./CodexOfficialAccount";
import WorkspaceCenter from "./WorkspaceCenter";
import { useCodexAuth } from "./useCodexAuth";

type AccountSummary = { id: number; email: string; username?: string | null; role?: string | null; balance: number; concurrency: number; status: string; runMode?: string | null };
type ApiKeySummary = { id: number; name: string; key?: string | null; status?: string | null; quota?: number | null; quotaUsed?: number | null; expiresAt?: string | null; groupId?: number | null; group?: GroupSummary | null };
type ModelSummary = { id: string; ownedBy?: string | null };
type Pagination<T> = { items: T[]; total: number };
type AppStateData = { session?: unknown | null; account?: AccountSummary | null; subscriptions: SubscriptionSummary[]; subscriptionProgress: SubscriptionProgressInfo[]; groups: GroupSummary[]; keys: Pagination<ApiKeySummary>; selectedTool: string; selectedKeyId?: number | null; loginWindowOpen: boolean; lastError?: string | null };
type ToolProfile = { tool: string; displayName: string; description: string; directory: string; configPath: string; notes?: string | null };
type SwitchTarget = { tool: string; profileName: string; baseUrl: string; apiKey: string; model?: string | null; reviewModel?: string | null; tokenType?: string | null; localRoutingEnabled?: boolean; localRouteApps?: string[]; localRouteModelMap?: Record<string, string>; localRoutePreserveClaudeAuth?: boolean; localRouteOnly?: boolean };
type LocalRouteEntry = { app: string; enabled: boolean; upstreamName: string; localBaseUrl: string; localApiKey: string; model?: string | null; modelMap?: Record<string, string>; source: string };
type LocalRouteManifest = { profileName: string; updatedAt: number; entries: LocalRouteEntry[] };
type LocalRouteStatus = { app: string; detected: boolean; configPath: string; baseUrlMatched: boolean; proxyKeyMatched: boolean; oauthPreserved?: boolean; mcpPreserved?: boolean; detail: string };
type EndpointProbeResult = { domain: string; baseUrl: string; attempts: number; successCount: number; packetLoss: number; averageLatencyMs?: number | null; bestLatencyMs?: number | null; selected: boolean; error?: string | null };
type EndpointProbeSummary = { selectedBaseUrl: string; selectedDomain: string; results: EndpointProbeResult[] };
type UpdateCheckResult = { currentVersion: string; latestVersion?: string | null; updateAvailable: boolean; releaseUrl?: string | null; downloadUrl?: string | null; downloadAccelerated?: boolean; mainlandChina?: boolean; repository: string; error?: string | null };
type UpdateInstallResult = { success: boolean; installerPath?: string | null; launched: boolean; message: string };
type UpdateDownloadProgress = { taskId: string; status: string; downloadedBytes: number; totalBytes: number; percent: number; message: string };
type ConfigSnapshotFile = { path: string; label: string; existed: boolean };
type ConfigSnapshotSummary = { id: string; createdAt: number; label: string; files: ConfigSnapshotFile[] };
type ConfigTransactionResult = { snapshot: ConfigSnapshotSummary; artifacts: [string, string][]; message: string };
type ConfigProfile = { id: string; createdAt: number; updatedAt: number; name: string; tool: string; baseUrl: string; keyId?: number | null; keyHint?: string | null; hasStoredKey: boolean; model?: string | null; reviewModel?: string | null; localRoutingEnabled: boolean; localRouteApps: string[]; localRouteModelMap: Record<string, string>; localRoutePreserveClaudeAuth: boolean; localRouteOnly: boolean };
type ConfigProfileInput = { name: string; tool: string; baseUrl: string; keyId?: number | null; apiKey?: string | null; model?: string | null; reviewModel?: string | null; localRoutingEnabled: boolean; localRouteApps: string[]; localRouteModelMap: Record<string, string>; localRoutePreserveClaudeAuth: boolean; localRouteOnly: boolean };
type OnboardingMode = "official" | "ai8888";
type AppPreferences = { onboardingCompleted: boolean; onboardingStep: number; onboardingMode?: OnboardingMode | null; dismissedAlertIds: string[] };
type MainView = "connections" | "switch" | "workspace" | "usage" | "settings";
type CodexSessionMeta = { sessionId: string; title?: string | null; summary?: string | null; projectDir?: string | null; createdAt?: string | null; lastActiveAt?: string | null; modelProvider?: string | null; modelProviderKey?: string | null; sourcePath: string; resumeCommand: string; archived: boolean; modifiedAt: number };
type CodexSessionSearchHit = { session: CodexSessionMeta; matchedIn: string[]; snippet?: string | null };
type CodexSessionMessage = { role: string; content: string; timestamp?: string | null };
type CodexSessionVisibilityRepairOutcome = { sessionId: string; sourcePath: string; success: boolean; changed: boolean; error?: string | null };
type UnifiedSessionMeta = { source: string; sourceLabel: string; sessionId: string; title?: string | null; summary?: string | null; projectDir?: string | null; createdAt?: string | null; lastActiveAt?: string | null; model?: string | null; sourcePath: string; resumeCommand?: string | null; archived: boolean; modifiedAt: number; messageCount?: number | null };
type UnifiedSessionMessage = { role: string; content: string; timestamp?: string | null; messageType?: string | null };

const defaultState: AppStateData = { account: null, subscriptions: [], subscriptionProgress: [], groups: [], keys: { items: [], total: 0 }, selectedTool: "codex", selectedKeyId: null, loginWindowOpen: false };
const mainViews: Array<{ id: MainView; label: string; description: string }> = [
  { id: "connections", label: "账户与连接", description: "OpenAI 官方账户、AI8888、订阅与 API Key" },
  { id: "switch", label: "配置切换", description: "连接方案、模型和本地路由" },
  { id: "workspace", label: "工作区", description: "项目、MCP、Prompts 与 Skills" },
  { id: "usage", label: "用量", description: "本地请求、Token 和成本" },
  { id: "settings", label: "设置", description: "代理容错、备份、诊断和更新" },
];
const onboardingStepLabels = ["选择使用方式", "连接账户", "自动准备", "完成设置"];

function safeDate(value?: string | null) { if (!value) return "-"; const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString(); }
function safeTimestamp(value: number) { const date = new Date(value); return Number.isNaN(date.getTime()) ? "-" : date.toLocaleString(); }
function bytesLabel(value: number) { if (!Number.isFinite(value) || value <= 0) return "0 B"; const units = ["B", "KB", "MB", "GB"]; const index = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024))); return `${(value / Math.pow(1024, index)).toFixed(index === 0 ? 0 : 1)} ${units[index]}`; }
function profileFingerprint(profile: ConfigProfile | ConfigProfileInput) {
  const modelMap = Object.fromEntries(Object.entries(profile.localRouteModelMap || {}).filter(([, value]) => Boolean(value?.trim())).sort(([left], [right]) => left.localeCompare(right)));
  return JSON.stringify({
    name: profile.name.trim(), tool: profile.tool, baseUrl: profile.baseUrl.replace(/\/$/, ""), keyId: profile.keyId ?? null,
    model: profile.model?.trim() || null, reviewModel: profile.reviewModel?.trim() || null, localRoutingEnabled: profile.localRoutingEnabled,
    localRouteApps: [...(profile.localRouteApps || [])].sort(), localRouteModelMap: modelMap,
    localRoutePreserveClaudeAuth: profile.localRoutePreserveClaudeAuth, localRouteOnly: profile.localRouteOnly,
  });
}
function maskKey(value?: string | null) { if (!value) return "\u672a\u8fd4\u56de\u660e\u6587"; if (value.length <= 12) return value; return `${value.slice(0, 8)}...${value.slice(-4)}`; }
function keyGroupId(key?: ApiKeySummary | null) { return key?.group?.id ?? key?.groupId ?? null; }
function keyGroup(key: ApiKeySummary | null | undefined, groups: GroupSummary[]) { return groups.find((group) => group.id === keyGroupId(key)) ?? key?.group ?? null; }
function isUsableApiKeyMetadata(key: ApiKeySummary) {
  const status = (key.status || "").trim().toLowerCase();
  if (["disabled", "revoked", "expired", "suspended", "inactive", "deleted"].includes(status)) return false;
  if (key.expiresAt) {
    const expiresAt = new Date(key.expiresAt).getTime();
    if (Number.isFinite(expiresAt) && expiresAt <= Date.now()) return false;
  }
  if (typeof key.quota === "number" && key.quota > 0 && typeof key.quotaUsed === "number" && key.quotaUsed >= key.quota) return false;
  return true;
}
function isUsableApiKey(key: ApiKeySummary) { return isUsableApiKeyMetadata(key) && Boolean(key.key?.trim()); }
function codexKeyCandidates(keys: ApiKeySummary[], groups: GroupSummary[]) {
  return keys.filter((key) => isUsableApiKey(key) && groupSupportsTool(keyGroup(key, groups), "codex"));
}
function allCodexSubscriptionGroupIds(subscriptions: SubscriptionSummary[], groups: GroupSummary[], progressList: SubscriptionProgressInfo[] = []) {
  const resolved = subscriptionsWithResolvedGroups(subscriptions, groups, progressList);
  return new Set(resolved.filter((subscription) => groupSupportsTool(subscription.group, "codex")).map((subscription) => subscription.group?.id ?? subscription.groupId).filter((groupId): groupId is number => typeof groupId === "number" && groupId > 0));
}
function preferredCodexKey(keys: ApiKeySummary[], groups: GroupSummary[], subscriptions: SubscriptionSummary[], progressList: SubscriptionProgressInfo[] = []) {
  const candidates = codexKeyCandidates(keys, groups);
  const resolved = subscriptionsWithResolvedGroups(subscriptions, groups, progressList);
  const activeIds = new Set(activeSubscriptionGroupIds(resolved, "codex"));
  if (activeIds.size > 0) return prioritizeByActiveSubscription(candidates, resolved, keyGroupId, "codex").find((key) => activeIds.has(keyGroupId(key) ?? -1)) ?? null;
  const subscriptionIds = allCodexSubscriptionGroupIds(resolved, groups);
  return candidates.find((key) => !subscriptionIds.has(keyGroupId(key) ?? -1)) ?? null;
}
function preferredBalanceGroupId(groups: GroupSummary[], subscriptions: SubscriptionSummary[], progressList: SubscriptionProgressInfo[] = []) {
  const subscriptionIds = allCodexSubscriptionGroupIds(subscriptions, groups, progressList);
  const candidates = groups.filter((group) => groupSupportsTool(group, "codex") && !subscriptionIds.has(group.id) && !["disabled", "inactive", "expired"].includes((group.status || "").toLowerCase()));
  const preferred = candidates.find((group) => /balance|standard|pay.?as.?you.?go|余额|普通/i.test(`${group.name} ${group.subscriptionType || ""}`));
  return preferred?.id ?? candidates[0]?.id ?? null;
}

function appLabel(app: string) { return app === "codex" ? "Codex" : app === "claude" ? "Claude" : app === "opencode" ? "OpenCode" : app; }

function endpointProbeText(result: EndpointProbeResult) {
  const loss = `${Math.round(result.packetLoss * 100)}%`;
  const avg = result.averageLatencyMs == null ? "-" : `${Math.round(result.averageLatencyMs)}ms`;
  return `${result.domain}：丢包 ${loss}，平均延迟 ${avg}`;
}


function isReloginError(text: string) {
  const value = (text || "").toLowerCase();
  return value.includes("无法获取账号信息")
    || value.includes("请重新登录")
    || value.includes("not logged in")
    || value.includes("missing refresh token")
    || value.includes("unauthorized")
    || value.includes("unauthenticated")
    || value.includes("invalid token")
    || value.includes("token expired")
    || value.includes("401")
    || value.includes("403");
}

function formatAuthError(err: unknown) {
  const text = err instanceof Error ? err.message : String(err);
  return isReloginError(text) ? "无法获取账号信息，请重新登录" : text;
}

function canUseDefaultModelAfterError(text: string) {
  const value = text.toLowerCase();
  if (["401", "402", "403", "unauthorized", "forbidden", "quota", "余额不足", "insufficient"].some((token) => value.includes(token))) return false;
  return value.includes("404") || value.includes("not found") || value.includes("method not allowed") || value.includes("models endpoint");
}

function Ai8888LoginPanel(props: { email: string; password: string; setEmail: (v: string) => void; setPassword: (v: string) => void; busy: boolean; onLogin: () => void; onOpenPurchase: () => void }) {
  return (
    <section className="panel connectionPanel">
      <div className="panelHead">
        <div>
          <h2>AI8888 账户</h2>
          <p className="muted sectionDescription">管理订阅额度、API Key 与 AI8888 配置方案</p>
        </div>
        <span className="badge">未登录</span>
      </div>
      <div className="inlineForm loginForm">
        <input value={props.email} onChange={(e) => props.setEmail(e.target.value)} placeholder="邮箱" />
        <input value={props.password} onChange={(e) => props.setPassword(e.target.value)} type="password" placeholder="密码" />
        <button onClick={props.onLogin} disabled={props.busy || !props.email || !props.password}>登录 AI8888</button>
      </div>
      <button className="link" onClick={props.onOpenPurchase}>充值或续费</button>
    </section>
  );
}


function sessionTitle(session: UnifiedSessionMeta) {
  return session.title || session.summary || session.projectDir?.split(/[\\/]/).filter(Boolean).pop() || session.sessionId;
}

function displayTime(value?: string | null) {
  if (!value) return "-";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function roleLabel(role: string) {
  const normalized = role.toLowerCase();
  if (normalized === "user") return "\u7528\u6237";
  if (normalized === "assistant") return "Assistant";
  if (normalized === "tool") return "\u5de5\u5177";
  return role || "\u6d88\u606f";
}

function CodexSessionsApp() {
  const [sessions, setSessions] = useState<UnifiedSessionMeta[]>([]);
  const [messages, setMessages] = useState<UnifiedSessionMessage[]>([]);
  const [selected, setSelected] = useState<UnifiedSessionMeta | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const [includeMessages, setIncludeMessages] = useState(true);
  const [scope, setScope] = useState<"all" | "active" | "archived">("all");
  const [sourceFilter, setSourceFilter] = useState("all");
  const [searching, setSearching] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("\u5c31\u7eea");

  function applySessions(next: UnifiedSessionMeta[]) {
    setSessions(next);
    setSelected((current) => {
      if (!current) return next[0] ?? null;
      return next.find((item) => item.sourcePath === current.sourcePath) ?? (next[0] ?? null);
    });
    setSelectedPaths((current) => {
      const valid = new Set(next.map((item) => item.sourcePath));
      return new Set(Array.from(current).filter((path) => valid.has(path)));
    });
  }

  async function loadSessions(okMessage?: string, searchQuery?: string) {
    setBusy(true); setError(null);
    try {
      const next = await invoke<UnifiedSessionMeta[]>("app_list_unified_sessions", { source: sourceFilter, query: searchQuery || null });
      applySessions(next);
      setMessage(okMessage ?? `已加载 ${next.length} 个跨工具会话`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function loadMessages(session: UnifiedSessionMeta | null) {
    if (!session) { setMessages([]); return; }
    setError(null);
    try {
      setMessages(await invoke<UnifiedSessionMessage[]>("app_get_unified_session_messages", { source: session.source, sourcePath: session.sourcePath }));
    } catch (err) {
      setMessages([]);
      setError(String(err));
    }
  }

  async function copy(text: string, ok: string) {
    await navigator.clipboard.writeText(text);
    setMessage(ok);
  }

  const filtered = useMemo(() => {
    let list = sessions;
    if (scope === "active") list = list.filter((session) => !session.archived);
    if (scope === "archived") list = list.filter((session) => session.archived);
    const text = query.trim().toLowerCase();
    if (!text) return list;
    return list.filter((session) => [session.sessionId, session.title, session.summary, session.projectDir, session.model, session.sourcePath, session.sourceLabel].some((value) => (value || "").toLowerCase().includes(text)));
  }, [query, sessions, scope]);

  async function runSessionSearch() {
    const text = query.trim();
    setSearching(true); setError(null);
    try {
      if (includeMessages && text) await loadSessions(undefined, text);
      else await loadSessions(text ? "已按会话元数据过滤" : "展示全部会话");
    } catch (err) {
      setError(String(err));
    } finally {
      setSearching(false);
    }
  }

  const selectedBatch = useMemo(() => sessions.filter((session) => selectedPaths.has(session.sourcePath)), [sessions, selectedPaths]);
  const actionTargets = selectedBatch.length > 0 ? selectedBatch : (selected ? [selected] : []);
  const allFilteredSelected = filtered.length > 0 && filtered.every((session) => selectedPaths.has(session.sourcePath));

  function toggleSelection(session: UnifiedSessionMeta, checked: boolean) {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (checked) next.add(session.sourcePath); else next.delete(session.sourcePath);
      return next;
    });
  }

  function toggleFilteredSelection() {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (allFilteredSelected) filtered.forEach((session) => next.delete(session.sourcePath));
      else filtered.forEach((session) => next.add(session.sourcePath));
      return next;
    });
  }

  async function launchSessions(targets: UnifiedSessionMeta[]) {
    if (targets.length === 0) return;
    setBusy(true); setError(null);
    const failed: string[] = [];
    const copied: string[] = [];
    try {
      for (const target of targets) {
        if (target.source === "codex") {
          try {
            await invoke("app_launch_codex_session", { sessionId: target.sessionId, cwd: target.projectDir ?? null, modelProviderKey: null });
          } catch (err) {
            failed.push(`${target.resumeCommand || target.sessionId}  # ${String(err)}`);
          }
        } else if (target.resumeCommand) {
          copied.push(target.resumeCommand);
        }
      }
      if (copied.length > 0) await navigator.clipboard.writeText(copied.join("\n"));
      if (failed.length > 0) {
        await navigator.clipboard.writeText(failed.join("\n"));
        setError(`\u6709 ${failed.length} \u4e2a\u4f1a\u8bdd\u542f\u52a8\u5931\u8d25\uff0c\u5df2\u590d\u5236\u5931\u8d25\u9879\u7684\u6062\u590d\u547d\u4ee4\u3002`);
      }
      const successCount = targets.filter((item) => item.source === "codex").length - failed.length;
      setMessage(`已打开 ${successCount} 个 Codex 恢复窗口${copied.length ? `，并复制 ${copied.length} 条其他工具恢复命令` : ""}`);
    } finally {
      setBusy(false);
    }
  }

  async function repairVisibility(targets: UnifiedSessionMeta[]) {
    const codexTargets = targets.filter((item) => item.source === "codex");
    if (codexTargets.length === 0) { setMessage("可见性修复仅适用于 Codex 会话"); return; }
    setBusy(true); setError(null);
    try {
      const results = await invoke<CodexSessionVisibilityRepairOutcome[]>("app_repair_codex_session_visibility", { requests: codexTargets.map((session) => ({ sessionId: session.sessionId, sourcePath: session.sourcePath })) });
      const failed = results.filter((item) => !item.success);
      const changed = results.filter((item) => item.success && item.changed).length;
      const unchanged = results.filter((item) => item.success && !item.changed).length;
      if (failed.length > 0) {
        setError(`\u6709 ${failed.length} \u4e2a\u4f1a\u8bdd\u4fee\u590d\u5931\u8d25\uff1a${failed[0].error || "\u672a\u77e5\u9519\u8bef"}`);
      }
      const next = await invoke<UnifiedSessionMeta[]>("app_list_unified_sessions", { source: sourceFilter, query: null });
      applySessions(next);
      setMessage(`\u5df2\u4fee\u590d ${changed} \u4e2a\u4f1a\u8bdd\u53ef\u89c1\u6027\uff0c${unchanged} \u4e2a\u65e0\u9700\u4fee\u590d`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => { void loadSessions(); }, [sourceFilter]);
  useEffect(() => { void loadMessages(selected); }, [selected?.sourcePath]);

  return (
    <main className="shell sessionsShell">
      <section className="hero sessionsHero">
        <div><p className="eyebrow">Sessions</p><h1>跨工具会话管理</h1><p className="heroText">浏览、搜索和恢复 Codex、Claude、Gemini、OpenCode、OpenClaw 与 Hermes 本地会话。</p></div>
        <div className="statusCard"><span className="dot ok" /><div><strong>{sessions.length}{" \u4e2a\u4f1a\u8bdd"}</strong><small>{message}</small></div></div>
      </section>
      {error && <div className="alert">{error}</div>}
      <section className="sessionsGrid">
        <aside className="panel sessionsListPanel">
          <div className="panelHead"><h2>{"\u4f1a\u8bdd\u5217\u8868"}</h2><button className="ghost mini" onClick={() => { void loadSessions(); }} disabled={busy || searching}>{"\u5237\u65b0"}</button></div>
          <div className="sessionSearchBox">
            <input value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") void runSessionSearch(); }} placeholder={"\u641c\u7d22\u4f1a\u8bdd ID / \u6807\u9898 / \u9879\u76ee / \u6d88\u606f\u5168\u6587"} />
            <div className="sessionFilters">
              <select value={sourceFilter} onChange={(e) => setSourceFilter(e.target.value)}><option value="all">全部工具</option><option value="codex">Codex</option><option value="claude">Claude</option><option value="gemini">Gemini</option><option value="opencode">OpenCode</option><option value="openclaw">OpenClaw</option><option value="hermes">Hermes</option></select>
              <select value={scope} onChange={(e) => { setScope(e.target.value as "all" | "active" | "archived"); }}>
                <option value="all">{"\u5168\u90e8"}</option>
                <option value="active">{"\u8fdb\u884c\u4e2d"}</option>
                <option value="archived">{"\u5df2\u5f52\u6863"}</option>
              </select>
              <label className="checkboxLine compact"><input type="checkbox" checked={includeMessages} onChange={(e) => setIncludeMessages(e.target.checked)} />{"\u6d88\u606f\u5168\u6587"}</label>
              <button className="secondary mini" onClick={() => void runSessionSearch()} disabled={busy || searching}>{searching ? "\u641c\u7d22\u4e2d" : "\u641c\u7d22"}</button>
            </div>
          </div>
          <div className="batchBar">
            <span>{"\u5df2\u9009 "}{selectedBatch.length}</span>
            <button className="ghost mini" onClick={toggleFilteredSelection} disabled={filtered.length === 0}>{allFilteredSelected ? "\u53d6\u6d88\u5f53\u524d" : "\u5168\u9009\u5f53\u524d"}</button>
            <button className="secondary mini" onClick={() => launchSessions(actionTargets)} disabled={busy || actionTargets.length === 0}>{"\u6062\u590d\u9009\u4e2d"}</button>
            <button className="ghost mini" onClick={() => repairVisibility(actionTargets)} disabled={busy || actionTargets.length === 0}>{"\u4fee\u590d\u53ef\u89c1\u6027"}</button>
          </div>
          <div className="list sessionsList">
            {filtered.length === 0 && <p className="muted">未找到会话。</p>}
            {filtered.map((session) => {
              const checked = selectedPaths.has(session.sourcePath);
              return <article className={"sessionItem " + (selected?.sourcePath === session.sourcePath ? "selected " : "") + (checked ? "checked" : "")} key={session.sourcePath} onClick={() => setSelected(session)}>
                <input aria-label="\u9009\u62e9\u4f1a\u8bdd" type="checkbox" checked={checked} onChange={(event) => toggleSelection(session, event.target.checked)} onClick={(event) => event.stopPropagation()} />
                <div>
                  <strong>{sessionTitle(session)}</strong>
                  <small>{session.sourceLabel} · {displayTime(session.lastActiveAt ?? session.createdAt)}{session.model ? ` · ${session.model}` : ""}{session.archived ? " · \u5df2\u5f52\u6863" : ""}</small>
                  <small>{session.projectDir || session.sessionId}</small>
                </div>
              </article>;
            })}
          </div>
        </aside>
        <section className="panel sessionDetailPanel">
          {!selected ? <div className="emptyState"><h2>{"\u9009\u62e9\u4e00\u4e2a\u4f1a\u8bdd"}</h2><p className="muted">选中会话后可查看消息和恢复命令。</p></div> : <>
            <div className="panelHead sessionDetailHead"><div><h2>{sessionTitle(selected)}</h2><p className="muted">{selected.sourceLabel} · {selected.projectDir || "\u672a\u8bb0\u5f55\u9879\u76ee\u76ee\u5f55"}{selected.model ? ` · ${selected.model}` : ""}</p></div><div className="actions"><button onClick={() => launchSessions([selected])} disabled={busy}>{selected.source === "codex" ? "恢复会话" : "复制恢复命令"}</button>{selected.resumeCommand && <button className="secondary" onClick={() => copy(selected.resumeCommand || "", "\u5df2\u590d\u5236\u6062\u590d\u547d\u4ee4")}>{"\u590d\u5236\u547d\u4ee4"}</button>}</div></div>
            <div className="resumeBox"><code>{selected.resumeCommand || "此来源暂不提供恢复命令"}</code><button className="ghost mini" onClick={() => copy(selected.sourcePath, "\u5df2\u590d\u5236\u6e90\u6587\u4ef6\u8def\u5f84")}>{"\u590d\u5236\u8def\u5f84"}</button></div>
            <div className="messageList">
              {messages.length === 0 && <p className="muted">{"\u6b64\u4f1a\u8bdd\u6ca1\u6709\u53ef\u663e\u793a\u7684\u6d88\u606f\u3002"}</p>}
              {messages.map((item, index) => <article className={`sessionMessage role-${item.role.toLowerCase()}`} key={`${item.timestamp || "m"}-${index}`}><header><strong>{roleLabel(item.role)}</strong><small>{displayTime(item.timestamp)}</small></header><pre>{item.content}</pre></article>)}
            </div>
          </>}
        </section>
      </section>
    </main>
  );
}function App() {
  const [state, setState] = useState<AppStateData>(defaultState);
  const [tools, setTools] = useState<ToolProfile[]>([]);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [baseUrl, setBaseUrl] = useState("https://sub.ai8888.shop");
  const [manualKey, setManualKey] = useState("");
  const [newKeyName, setNewKeyName] = useState("AI8888 Switch");
  const [newKeyGroupId, setNewKeyGroupId] = useState("");
  const [editKeyGroupId, setEditKeyGroupId] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);
  const [ai8888DataReady, setAi8888DataReady] = useState(false);
  const [message, setMessage] = useState("就绪");
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<[string, string][]>([]);
  const [models, setModels] = useState<ModelSummary[]>([]);
  const [selectedModel, setSelectedModel] = useState("");
  const [selectedReviewModel, setSelectedReviewModel] = useState("");
  const [testingModels, setTestingModels] = useState(false);
  const [probingEndpoint, setProbingEndpoint] = useState(false);
  const [endpointProbe, setEndpointProbe] = useState<EndpointProbeSummary | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateCheckResult | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateDownloadProgress | null>(null);
  const [configSnapshots, setConfigSnapshots] = useState<ConfigSnapshotSummary[]>([]);
  const [restoringSnapshotId, setRestoringSnapshotId] = useState<string | null>(null);
  const [profiles, setProfiles] = useState<ConfigProfile[]>([]);
  const [selectedProfileId, setSelectedProfileId] = useState<string | null>(null);
  const [profileNameDraft, setProfileNameDraft] = useState("");
  const [profileBusyId, setProfileBusyId] = useState<string | null>(null);
  const [preferences, setPreferences] = useState<AppPreferences>({ onboardingCompleted: true, onboardingStep: 0, onboardingMode: null, dismissedAlertIds: [] });
  const [showWizard, setShowWizard] = useState(false);
  const [wizardSaving, setWizardSaving] = useState(false);
  const [wizardModelAttempted, setWizardModelAttempted] = useState(false);
  const wizardModelRequestRef = useRef(false);
  const [wizardModeDraft, setWizardModeDraft] = useState<OnboardingMode>("official");
  const wizardAutoAdvanceRef = useRef(false);
  const [routeCodexEnabled, setRouteCodexEnabled] = useState(false);
  const [routeClaudeEnabled, setRouteClaudeEnabled] = useState(false);
  const [routeOpenCodeEnabled, setRouteOpenCodeEnabled] = useState(false);
  const [localRouteModelMap, setLocalRouteModelMap] = useState<Record<string, string>>({ sonnet: "", opus: "", haiku: "" });
  const [localRoutePreserveClaudeAuth, setLocalRoutePreserveClaudeAuth] = useState(false);
  const [localRouteOnly, setLocalRouteOnly] = useState(false);
  const [localRouteManifest, setLocalRouteManifest] = useState<LocalRouteManifest | null>(null);
  const [localRouteStatuses, setLocalRouteStatuses] = useState<LocalRouteStatus[]>([]);
  const [activeView, setActiveView] = useState<MainView>(() => {
    try {
      const saved = window.localStorage.getItem("ai8888-switch.active-view");
      return mainViews.some((item) => item.id === saved) ? saved as MainView : "connections";
    } catch {
      return "connections";
    }
  });
  const codexAuth = useCodexAuth({
    onConfigChanged: async () => {
      await refreshConfigSnapshots();
      await refreshLocalRouteManifest();
    },
  });

  const selectedTool = useMemo(() => tools.find((tool) => tool.tool === state.selectedTool) ?? tools[0], [state.selectedTool, tools]);
  const selectedKey = useMemo(() => state.selectedKeyId == null ? null : (state.keys.items.find((key) => key.id === state.selectedKeyId) ?? null), [state.keys.items, state.selectedKeyId]);
  const selectedProfile = useMemo(() => profiles.find((profile) => profile.id === selectedProfileId) ?? null, [profiles, selectedProfileId]);
  const isAuthenticated = Boolean(state.session);
  const accountAlerts = useMemo(() => {
    if (!isAuthenticated) return [];
    const all = buildAccountAlerts({ balance: state.account?.balance, subscriptions: state.subscriptions, subscriptionProgress: state.subscriptionProgress });
    const dismissed = new Set(preferences.dismissedAlertIds || []);
    return all.filter((alert) => !dismissed.has(alert.id));
  }, [isAuthenticated, state.account?.balance, state.subscriptions, state.subscriptionProgress, preferences.dismissedAlertIds]);

  const onboardingMode: OnboardingMode = preferences.onboardingMode === "ai8888" || preferences.onboardingMode === "official"
    ? preferences.onboardingMode
    : preferences.onboardingStep > 0 && isAuthenticated ? "ai8888" : "official";
  const onboardingStep = Math.min(3, Math.max(0, preferences.onboardingStep || 0));
  const resolvedSubscriptions = useMemo(() => subscriptionsWithResolvedGroups(state.subscriptions, state.groups, state.subscriptionProgress), [state.subscriptions, state.groups, state.subscriptionProgress]);
  const subscriptionGroupIds = useMemo(() => new Set(activeSubscriptionGroupIds(resolvedSubscriptions, "codex")), [resolvedSubscriptions]);
  const wizardKeys = useMemo(() => codexKeyCandidates(state.keys.items, state.groups), [state.keys.items, state.groups]);
  const recommendedSubscriptionGroup = recommendedSubscriptionGroupId(resolvedSubscriptions, "codex");
  const recommendedSubscription = recommendedSubscriptionGroup == null
    ? null
    : resolvedSubscriptions.find((subscription) => (subscription.group?.id ?? subscription.groupId) === recommendedSubscriptionGroup) ?? null;
  const recommendedSubscriptionName = recommendedSubscription?.group?.name || recommendedSubscription?.groupName || "可用套餐";
  const recommendedSubscriptionExpiry = recommendedSubscription?.expiresAt ? `，有效至 ${safeDate(recommendedSubscription.expiresAt)}` : "";
  const recommendedKey = useMemo(() => preferredCodexKey(state.keys.items, state.groups, state.subscriptions, state.subscriptionProgress), [state.keys.items, state.groups, state.subscriptions, state.subscriptionProgress]);
  const recommendedKeyUsesSubscription = Boolean(recommendedKey && subscriptionGroupIds.has(keyGroupId(recommendedKey) ?? -1));
  const usableSelectedKey = selectedKey && wizardKeys.some((item) => item.id === selectedKey.id) ? selectedKey : null;
  const effectiveKey = manualKey.trim() || usableSelectedKey?.key || "";
  const wizardSelectedKey = usableSelectedKey && manualKey.trim() === (usableSelectedKey.key?.trim() || "") ? usableSelectedKey : null;
  const wizardSubscription = wizardSelectedKey
    ? resolvedSubscriptions.find((subscription) => (subscription.group?.id ?? subscription.groupId) === keyGroupId(wizardSelectedKey)) ?? recommendedSubscription
    : recommendedSubscription;
  const wizardSubscriptionName = wizardSubscription?.group?.name || wizardSubscription?.groupName || recommendedSubscriptionName;
  const wizardFundingSource = wizardSelectedKey && subscriptionGroupIds.has(keyGroupId(wizardSelectedKey) ?? -1)
    ? "subscription"
    : wizardSelectedKey && !allCodexSubscriptionGroupIds(resolvedSubscriptions, state.groups).has(keyGroupId(wizardSelectedKey) ?? -1) && Number(state.account?.balance ?? 0) > 0
      ? "balance"
      : null;
  const hasAi8888Funding = ai8888DataReady && (recommendedSubscriptionGroup != null || Number(state.account?.balance ?? 0) > 0);
  const expectedWizardFundingSource = recommendedSubscriptionGroup != null ? "subscription" : "balance";
  const wizardDisplayMode: OnboardingMode = onboardingStep === 0 ? wizardModeDraft : onboardingMode;
  const modelChoice = selectedModel.trim();
  const reviewModelChoice = selectedReviewModel.trim();
  const canWrite = onboardingMode === "official"
    ? Boolean(codexAuth.status?.authenticated)
    : showWizard
      ? Boolean(state.session && ai8888DataReady && wizardSelectedKey && wizardFundingSource === expectedWizardFundingSource && baseUrl.trim())
      : Boolean(state.session && effectiveKey && baseUrl.trim());
  const localRoutingEnabled = routeCodexEnabled || routeClaudeEnabled || routeOpenCodeEnabled;
  const localRouteApps = useMemo(() => {
    const apps: string[] = [];
    if (routeCodexEnabled) apps.push("codex");
    if (routeClaudeEnabled) apps.push("claude");
    if (routeOpenCodeEnabled) apps.push("opencode");
    return apps;
  }, [routeCodexEnabled, routeClaudeEnabled, routeOpenCodeEnabled]);
  const activeViewMeta = mainViews.find((item) => item.id === activeView) ?? mainViews[0];
  const currentProfileKeyId = useMemo(() => {
    if (!selectedKey) return null;
    const entered = manualKey.trim();
    const selectedValue = selectedKey.key?.trim() || "";
    return entered && entered !== selectedValue ? null : selectedKey.id;
  }, [manualKey, selectedKey]);
  const currentProfileInput = useMemo<ConfigProfileInput>(() => ({
    name: profileNameDraft.trim(),
    tool: state.selectedTool,
    baseUrl,
    keyId: currentProfileKeyId,
    apiKey: currentProfileKeyId == null ? (manualKey.trim() || null) : null,
    model: modelChoice || null,
    reviewModel: reviewModelChoice || null,
    localRoutingEnabled,
    localRouteApps,
    localRouteModelMap,
    localRoutePreserveClaudeAuth,
    localRouteOnly,
  }), [profileNameDraft, state.selectedTool, baseUrl, currentProfileKeyId, manualKey, modelChoice, reviewModelChoice, localRoutingEnabled, localRouteApps, localRouteModelMap, localRoutePreserveClaudeAuth, localRouteOnly]);
  const profileDirty = Boolean(selectedProfile && (
    profileFingerprint(currentProfileInput) !== profileFingerprint(selectedProfile)
    || (selectedProfile.keyId == null && Boolean(manualKey.trim()))
  ));
  const profileFormHasUnsavedChanges = profileDirty || Boolean(!selectedProfile && (manualKey.trim() || modelChoice || reviewModelChoice || localRoutingEnabled));

  async function run<T>(action: () => Promise<T>, okMessage: string) {
    setBusy(true); setError(null);
    try {
      const result = await action();
      setMessage(okMessage);
      return result;
    } catch (err) {
      const text = formatAuthError(err);
      setError(text);
      setMessage(isReloginError(text) ? "请重新登录" : "操作失败");
      if (isReloginError(text)) {
        setState({ ...defaultState });
        setManualKey("");
        setModels([]);
        setSelectedModel("");
        setSelectedReviewModel("");
        setModelError(null);
      }
      throw err;
    } finally {
      setBusy(false);
    }
  }

  async function refreshLocalState() { const next = await invoke<AppStateData>("app_get_state"); setState({ ...defaultState, ...next }); }
  async function refreshLocalRouteManifest() { try { const [next, statuses] = await Promise.all([invoke<LocalRouteManifest>("app_get_local_route_manifest").catch(() => null), invoke<LocalRouteStatus[]>("app_get_local_route_statuses")]); setLocalRouteManifest(next); setLocalRouteStatuses(statuses); } catch { setLocalRouteManifest(null); setLocalRouteStatuses([]); } }
  const refreshRemote = useCallback(async (options?: { silent?: boolean }) => {
    const silent = Boolean(options?.silent);
    try {
      if (silent) {
        const next = await invoke<AppStateData>("app_load_remote_state");
        setState({ ...defaultState, ...next });
        setAi8888DataReady(true);
        setError(null);
      } else {
        const next = await run(() => invoke<AppStateData>("app_load_remote_state"), "\u5df2\u5237\u65b0\u4f59\u989d\u3001\u8ba2\u9605\u4e0e Key");
        if (next) { setState({ ...defaultState, ...next }); setAi8888DataReady(true); setError(null); }
      }
    } catch (err) {
      const text = formatAuthError(err);
      if (isReloginError(text)) {
        setState({ ...defaultState });
        setAi8888DataReady(false);
        setManualKey("");
        setModels([]);
        setSelectedModel("");
        setSelectedReviewModel("");
        setModelError(null);
        setError(text);
        setMessage("\u8bf7\u91cd\u65b0\u767b\u5f55");
        return;
      }
      setAi8888DataReady(false);
      if (!silent) {
        setError(text);
        setMessage("\u64cd\u4f5c\u5931\u8d25");
      }
    }
  }, []);
  async function chooseTool(tool: string) {
    try {
      const next = await invoke<AppStateData>("app_set_selected_tool", { tool });
      setState({ ...defaultState, ...next });
      setModels([]); setSelectedModel(""); setSelectedReviewModel(""); setModelError(null); setWizardModelAttempted(false); wizardModelRequestRef.current = false;
      return true;
    } catch (err) {
      setError(formatAuthError(err));
      return false;
    }
  }
  async function chooseKey(keyId: number) {
    try {
      const next = await invoke<AppStateData>("app_set_selected_key", { keyId });
      setState({ ...defaultState, ...next });
      const selectedItem = next.keys.items.find((item) => item.id === keyId);
      setNewKeyGroupId(keyGroupId(selectedItem)?.toString() ?? "");
      setManualKey(selectedItem?.key ?? "");
      setModels([]); setSelectedModel(""); setSelectedReviewModel(""); setModelError(null); setWizardModelAttempted(false); wizardModelRequestRef.current = false;
      return Boolean(selectedItem?.key);
    } catch (err) {
      setError(formatAuthError(err));
      return false;
    }
  }
  async function updateKeyGroup(keyId: number, groupId: string) { await run(() => invoke("app_update_key_group", { keyId, groupId: groupId ? Number(groupId) : null }), "已更新 Key 分组"); await refreshLocalState(); }
  async function createKey(options?: { groupId?: number | null; name?: string }): Promise<ApiKeySummary | null> {
    try {
      const groupId = options && "groupId" in options ? options.groupId ?? null : (newKeyGroupId ? Number(newKeyGroupId) : null);
      const created = await run(() => invoke<ApiKeySummary>("app_create_key", { payload: { name: options?.name || newKeyName, groupId } }), "API Key 已创建");
      if (!created?.key) {
        setError("Key 已创建，但服务端没有返回一次性密钥，请刷新账户后重试。");
        await refreshLocalState();
        return null;
      }
      let resolvedCreated = created;
      if (groupId != null && keyGroupId(created) !== groupId) {
        try {
          const refreshed = await invoke<AppStateData>("app_load_remote_state");
          setState({ ...defaultState, ...refreshed });
          const verified = refreshed.keys.items.find((item) => item.id === created.id);
          if (!verified || keyGroupId(verified) !== groupId) {
            setError("服务端没有确认 Key 的订阅分组，未继续使用该 Key");
            return null;
          }
          resolvedCreated = { ...verified, key: verified.key || created.key };
        } catch (reason) {
          setError(`无法确认 Key 的分组：${formatAuthError(reason)}`);
          return null;
        }
      }
      if (options && !groupSupportsTool(keyGroup(resolvedCreated, state.groups), "codex")) {
        setError("服务端返回的 Key 不是 Codex 可用分组，未继续使用该 Key");
        return null;
      }
      setManualKey(resolvedCreated.key || "");
      try {
        const next = await invoke<AppStateData>("app_set_selected_key", { keyId: resolvedCreated.id });
        setState({ ...defaultState, ...next });
      } catch (err) {
        setError(`Key 已创建，但暂时无法选中：${formatAuthError(err)}`);
      }
      return resolvedCreated;
    } catch {
      return null;
    }
  }
  async function deleteKey(keyId: number) {
    const item = state.keys.items.find((key) => key.id === keyId);
    if (!window.confirm(`确认删除 API Key「${item?.name || `#${keyId}`}」？`)) return;
    await run(() => invoke("app_delete_key", { keyId }), "API Key 已删除");
    if (state.selectedKeyId === keyId) { setManualKey(""); setModels([]); setSelectedModel(""); setSelectedReviewModel(""); }
    await refreshLocalState();
  }
  async function login(): Promise<boolean> {
    try {
      const next = await run(() => invoke<AppStateData>("app_login_with_password", { email, password }), "登录成功");
      if (!next) return false;
      const preferredGroupId = recommendedSubscriptionGroupId(next.subscriptions);
      setNewKeyGroupId(preferredGroupId?.toString() ?? next.keys.items[0]?.group?.id?.toString() ?? "");
      let resolvedState = next;
      if (!preferences.onboardingCompleted && onboardingMode === "ai8888") {
        const preferredKey = preferredCodexKey(next.keys.items, next.groups, next.subscriptions, next.subscriptionProgress);
        if (preferredKey) {
          resolvedState = await invoke<AppStateData>("app_set_selected_key", { keyId: preferredKey.id });
          setManualKey(preferredKey.key ?? "");
        }
      }
      setState({ ...defaultState, ...resolvedState });
      setAi8888DataReady(true);
      setPassword("");
      await refreshLocalRouteManifest();
      if (!preferences.onboardingCompleted) { setShowWizard(true); setActiveView("switch"); }
      return true;
    } catch {
      return false;
    }
  }
  async function logout() {
    const next = await run(() => invoke<AppStateData>("app_logout"), "已退出登录");
    if (next) {
      const pending = !preferences.onboardingCompleted;
      setState({ ...defaultState, ...next }); setAi8888DataReady(false); setManualKey(""); setModels([]); setSelectedModel(""); setSelectedReviewModel(""); setModelError(null);
      setShowWizard(pending);
      if (pending) {
        setActiveView("switch"); setWizardModeDraft(onboardingMode);
        if (onboardingMode === "ai8888") await persistOnboardingStep(1, "ai8888");
      }
    }
  }
  async function openPurchase() { await run(() => invoke("app_open_purchase_window"), "已打开充值续费页面"); }
  async function openDailyReset() { await run(() => invoke("app_open_daily_reset_window"), "已打开日卡重置页面"); }
  async function openRadar() { await run(() => invoke("app_open_radar_window"), "已打开智商雷达"); }
  async function openModelStatus() { await run(() => invoke("app_open_model_status_window"), "已打开模型监控"); }

  async function openCodexSessions() { await run(() => invoke("app_open_codex_sessions_window"), "\u5df2\u6253\u5f00 Codex \u4f1a\u8bdd\u7ba1\u7406"); }


  async function activateAi8888ForCodex() {
    if (!effectiveKey) throw new Error("请先选择或填写一个可用的 AI8888 Key");
    const target = await invoke<SwitchTarget>("app_prepare_switch", {
      tool: "codex",
      baseUrl,
      apiKey: effectiveKey,
      model: modelChoice || null,
      reviewModel: reviewModelChoice || null,
      localRoutingEnabled: false,
      localRouteApps: [],
      localRouteModelMap: {},
      localRoutePreserveClaudeAuth: false,
      localRouteOnly: false,
    });
    const result = await invoke<ConfigTransactionResult>("app_write_switch", { target });
    setPreview(result.artifacts);
    setMessage("Codex 已切换到 AI8888");
    await refreshConfigSnapshots();
    await refreshLocalRouteManifest();
  }

    const checkUpdate = useCallback(async (options?: { silent?: boolean }) => {
    const silent = Boolean(options?.silent);
    setCheckingUpdate(true);
    if (!silent) setError(null);
    try {
      const result = await invoke<UpdateCheckResult>("app_check_update");
      setUpdateInfo(result);
      if (result.error) {
        if (!silent) setMessage(`更新检查失败：${result.error}`);
      } else if (result.updateAvailable) {
        setMessage(result.downloadAccelerated
          ? `发现新版本 ${result.latestVersion}，已切换 GitHub 加速下载`
          : `发现新版本 ${result.latestVersion}`);
      } else if (!silent) {
        setMessage(`当前已是最新版本 ${result.currentVersion}`);
      }
    } catch (err) {
      if (!silent) setError(String(err));
    } finally {
      setCheckingUpdate(false);
    }
  }, []);

  useEffect(() => {
    const FOUR_HOURS_MS = 4 * 60 * 60 * 1000;
    // Auto-check once on every app open.
    void checkUpdate({ silent: true });
    const timer = window.setInterval(() => {
      void checkUpdate({ silent: true });
    }, FOUR_HOURS_MS);
    return () => window.clearInterval(timer);
  }, [checkUpdate]);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<UpdateDownloadProgress>("update-download-progress", (event) => {
      setUpdateProgress(event.payload);
      if (["completed", "canceled", "failed"].includes(event.payload.status)) {
        setInstallingUpdate(false);
      }
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    if (!isAuthenticated) return;
    const FIFTEEN_MINUTES_MS = 15 * 60 * 1000;
    // Keep usage/expiry alerts fresh while logged in.
    void refreshRemote({ silent: true });
    const timer = window.setInterval(() => {
      void refreshRemote({ silent: true });
    }, FIFTEEN_MINUTES_MS);

    const onVisible = () => {
      if (document.visibilityState === "visible") {
        void refreshRemote({ silent: true });
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onVisible);

    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onVisible);
    };
  }, [isAuthenticated, refreshRemote]);


  async function installUpdate() {
    if (!updateInfo?.downloadUrl || !updateInfo.latestVersion) {
      setMessage("\u6ca1\u6709\u53ef\u5b89\u88c5\u7684\u66f4\u65b0\u8d44\u6e90");
      return;
    }
    setInstallingUpdate(true); setError(null); setUpdateProgress({ taskId: "", status: "preparing", downloadedBytes: 0, totalBytes: 0, percent: 0, message: "正在准备更新" });
    try {
      const result = await invoke<UpdateInstallResult>("app_install_update", {
        version: updateInfo.latestVersion,
        preferAccelerated: Boolean(updateInfo.downloadAccelerated),
      });
      if (result.success) setMessage(result.message);
      else setError(result.message || "\u5b89\u88c5\u5931\u8d25");
    } catch (err) {
      const text = String(err);
      if (text.toLowerCase().includes("canceled")) setMessage("更新下载已取消");
      else setError(text);
    } finally {
      setInstallingUpdate(false);
    }
  }

  async function cancelUpdate() {
    try {
      const canceled = await invoke<boolean>("app_cancel_update");
      setMessage(canceled ? "正在取消更新下载" : "当前没有进行中的更新下载");
    } catch (err) {
      setError(String(err));
    }
  }

  async function dismissAlert(alertId: string) {
    try {
      const next = await invoke<AppPreferences>("app_dismiss_alert", { alertId });
      setPreferences(next);
    } catch (err) {
      setError(String(err));
    }
  }

  async function completeOnboarding(): Promise<boolean> {
    try {
      const next = await invoke<AppPreferences>("app_complete_onboarding");
      setPreferences(next);
      setShowWizard(false);
      setMessage("首次设置已完成，配置已经可以使用");
      return true;
    } catch (err) {
      setError(String(err));
      return false;
    }
  }

  async function persistOnboardingStep(step: number, mode: OnboardingMode | null = onboardingMode): Promise<boolean> {
    const next = { ...preferences, onboardingCompleted: false, onboardingStep: step, onboardingMode: mode, dismissedAlertIds: preferences.dismissedAlertIds || [] };
    try {
      const saved = await invoke<AppPreferences>("app_set_preferences", { preferences: next });
      setPreferences(saved);
      return true;
    } catch (err) {
      setError(String(err));
      return false;
    }
  }

  async function saveOnboardingStep(step: number) {
    if (wizardSaving) return false;
    setWizardSaving(true); setError(null);
    try { return await persistOnboardingStep(step); }
    finally { setWizardSaving(false); }
  }

  async function stopWizardOfficialLogin() {
    if (onboardingMode !== "official" || !codexAuth.status?.loginRunning) return true;
    return Boolean(await codexAuth.cancelLogin());
  }

  async function goBackOnboarding() {
    if (!(await stopWizardOfficialLogin())) return;
    await saveOnboardingStep(onboardingStep - 1);
  }

  async function deferOnboarding() {
    if (!(await stopWizardOfficialLogin())) return;
    if (await saveOnboardingStep(onboardingStep)) {
      setShowWizard(false);
      setMessage("已暂时跳过首次设置，下次启动仍可继续");
    }
  }

  async function startWizardDeviceLogin() {
    const started = await codexAuth.startLogin("device", false);
    if (started) await codexAuth.openDevicePage();
  }

  async function restartOnboarding() {
    if (wizardSaving) return;
    if (!(await stopWizardOfficialLogin())) return;
    setWizardSaving(true); setError(null);
    try {
      if (await persistOnboardingStep(0, null)) {
        setWizardModeDraft("official");
        setWizardModelAttempted(false); wizardModelRequestRef.current = false;
        setShowWizard(true); setActiveView("switch");
      }
    } finally {
      setWizardSaving(false);
    }
  }

  async function advanceOnboarding() {
    if (wizardSaving) return;
    setWizardSaving(true); setError(null);
    try {
      if (onboardingStep === 0) {
        if (!(await chooseTool("codex"))) return;
        await persistOnboardingStep(1, wizardModeDraft);
        return;
      }
      if (onboardingStep === 1) {
        if (onboardingMode === "official") {
          const status = codexAuth.status ?? await codexAuth.refresh();
          if (!status?.cliAvailable) { setError("尚未找到 Codex CLI，请重新检测后再继续"); return; }
          if (!status.authenticated) { setError("请先使用 ChatGPT 登录，登录成功后会自动继续"); return; }
          await persistOnboardingStep(2, "official");
          return;
        }

        if (!isAuthenticated) { setError("请先登录 AI8888 账户"); return; }
        if (!ai8888DataReady) { setError("暂时无法确认订阅状态，请先刷新账户后重试"); return; }
        if (!hasAi8888Funding) { setError("没有可用订阅或余额，请先充值/续费"); return; }
        let resolvedBaseUrl = endpointProbe?.selectedBaseUrl || baseUrl.trim();
        if (!endpointProbe) {
          const detected = await probeBestEndpoint();
          if (detected) resolvedBaseUrl = detected.selectedBaseUrl;
          else if (!resolvedBaseUrl) return;
          else { setError(null); setMessage("测速未完成，将使用默认线路继续"); }
        }
        if (!resolvedBaseUrl) { setError("网络线路尚未准备好，请重新检测"); return; }

        let resolvedKey = "";
        const subscriptionKey = recommendedKeyUsesSubscription ? recommendedKey : null;
        let selectedForWizard: ApiKeySummary | null | undefined = subscriptionKey ?? recommendedKey;
        if (recommendedSubscriptionGroup != null && !subscriptionKey) {
          setMessage("检测到有效订阅，正在创建订阅专用访问密钥");
          selectedForWizard = await createKey({ groupId: recommendedSubscriptionGroup, name: "AI8888 Subscription" });
        } else if (!selectedForWizard) {
          setMessage("未检测到订阅，正在创建余额访问密钥");
          selectedForWizard = await createKey({ groupId: preferredBalanceGroupId(state.groups, state.subscriptions, state.subscriptionProgress), name: "AI8888 Balance" });
        } else if (selectedKey?.id !== selectedForWizard.id || manualKey.trim() !== (selectedForWizard.key?.trim() || "")) {
          if (!(await chooseKey(selectedForWizard.id))) return;
        }
        resolvedKey = selectedForWizard?.key?.trim() || "";
        if (!resolvedKey) { setError("访问密钥准备失败，请重试"); return; }
        const selectedGroupId = keyGroupId(selectedForWizard);
        if (recommendedSubscriptionGroup != null && !subscriptionGroupIds.has(selectedGroupId ?? -1)) {
          setError("订阅访问密钥绑定异常，请刷新账户后重试");
          return;
        }
        if (recommendedSubscriptionGroup == null && allCodexSubscriptionGroupIds(resolvedSubscriptions, state.groups).has(selectedGroupId ?? -1)) {
          setError("当前 Key 属于已结束订阅，请重试以创建余额 Key");
          return;
        }

        if (!(await testModels(resolvedBaseUrl, resolvedKey))) return;
        await persistOnboardingStep(2, "ai8888");
        return;
      }
      if (onboardingStep === 2) {
        if (onboardingMode === "ai8888" && (!isAuthenticated || !ai8888DataReady || !hasAi8888Funding || !effectiveKey || !baseUrl.trim() || wizardFundingSource !== expectedWizardFundingSource)) {
          await persistOnboardingStep(1, "ai8888");
          setMessage("连接状态已变化，请重新自动准备");
          return;
        }
        if (onboardingMode === "ai8888" && modelError) {
          setError("连接验证失败，请重试读取模型后再继续");
          return;
        }
        await persistOnboardingStep(3, onboardingMode);
      }
    } finally {
      setWizardSaving(false);
      if (onboardingStep === 1) wizardAutoAdvanceRef.current = false;
    }
  }

  async function finishOnboarding() {
    if (wizardSaving || busy || codexAuth.busy) return;
    if (!canWrite) { setError(onboardingMode === "official" ? "OpenAI 官方账户尚未登录" : "连接尚未准备完成，请返回上一步重新自动准备"); return; }
    setWizardSaving(true); setError(null);
    try {
      if (onboardingMode === "official") {
        if (await codexAuth.activateOfficial()) await completeOnboarding();
      } else {
        try {
          const refreshed = await invoke<AppStateData>("app_load_remote_state");
          const refreshedSubscriptions = subscriptionsWithResolvedGroups(refreshed.subscriptions, refreshed.groups, refreshed.subscriptionProgress);
          const activeGroupIds = new Set(activeSubscriptionGroupIds(refreshedSubscriptions, "codex"));
          const allSubscriptionIds = allCodexSubscriptionGroupIds(refreshedSubscriptions, refreshed.groups);
          const refreshedKey = refreshed.keys.items.find((item) => item.id === wizardSelectedKey?.id) ?? null;
          const refreshedGroupId = keyGroupId(refreshedKey);
          const expectedSource = activeGroupIds.size > 0 ? "subscription" : "balance";
          const actualSource = refreshedKey && activeGroupIds.has(refreshedGroupId ?? -1)
            ? "subscription"
            : refreshedKey && !allSubscriptionIds.has(refreshedGroupId ?? -1) && Number(refreshed.account?.balance ?? 0) > 0
              ? "balance"
              : null;
          if (!refreshedKey || !isUsableApiKeyMetadata(refreshedKey) || !groupSupportsTool(keyGroup(refreshedKey, refreshed.groups), "codex") || actualSource !== expectedSource) {
            setState({ ...defaultState, ...refreshed });
            setAi8888DataReady(true);
            setManualKey("");
            await persistOnboardingStep(1, "ai8888");
            setError("订阅或访问密钥状态已变化，已返回重新自动准备");
            return;
          }
          setAi8888DataReady(true);
          if (await writeSwitch()) {
            const keys = { ...refreshed.keys, items: refreshed.keys.items.map((item) => item.id === refreshedKey.id ? { ...item, key: item.key || manualKey } : item) };
            setState({ ...defaultState, ...refreshed, keys });
            await completeOnboarding();
          }
        } catch (reason) {
          setAi8888DataReady(false);
          await persistOnboardingStep(1, "ai8888");
          setError(`无法再次确认订阅状态：${formatAuthError(reason)}`);
        }
      }
    } finally {
      setWizardSaving(false);
    }
  }

  async function testModels(resolvedBaseUrl = baseUrl, resolvedKey = effectiveKey): Promise<boolean> {
    setWizardModelAttempted(true);
    setTestingModels(true); setModelError(null);
    try { const next = await invoke<ModelSummary[]>("app_fetch_models", { query: { baseUrl: resolvedBaseUrl, apiKey: resolvedKey, isFullUrl: false, modelsUrlOverride: null, userAgent: null } }); setModels(next); setSelectedModel((current) => current && next.some((item) => item.id === current) ? current : (next[0]?.id ?? "")); setMessage(`已获取 ${next.length} 个模型`); return true; }
    catch (err) {
      const text = err instanceof Error ? err.message : String(err);
      setModels([]);
      if (canUseDefaultModelAfterError(text)) {
        setModelError(null);
        setMessage("服务端没有提供模型列表，将使用默认模型");
        return true;
      }
      setModelError(text);
      return false;
    }
    finally { setTestingModels(false); }
  }

  function retryWizardModels() {
    wizardModelRequestRef.current = false;
    setWizardModelAttempted(false);
    setModelError(null);
  }

  async function probeBestEndpoint(): Promise<EndpointProbeSummary | null> {
    setProbingEndpoint(true); setError(null); setEndpointProbe(null);
    try {
      const summary = await invoke<EndpointProbeSummary>("app_probe_best_endpoint");
      setEndpointProbe(summary);
      setBaseUrl(summary.selectedBaseUrl);
      const picked = summary.results.find((item) => item.selected);
      const loss = picked ? `${Math.round(picked.packetLoss * 100)}%` : "-";
      const latency = picked?.averageLatencyMs == null ? "-" : `${Math.round(picked.averageLatencyMs)}ms`;
      setMessage(`已选择最优端点 ${summary.selectedDomain}（丢包 ${loss}，延迟 ${latency}）`);
      return summary;
    } catch (err) {
      const text = err instanceof Error ? err.message : String(err);
      setError(text);
      setMessage("端点检测失败");
      return null;
    } finally {
      setProbingEndpoint(false);
    }
  }

  function prepareTarget(): Promise<SwitchTarget> {
    return invoke<SwitchTarget>("app_prepare_switch", { tool: state.selectedTool, baseUrl, apiKey: effectiveKey, model: modelChoice || null, reviewModel: reviewModelChoice || null, localRoutingEnabled, localRouteApps, localRouteModelMap, localRoutePreserveClaudeAuth, localRouteOnly });
  }

  async function refreshProfiles(preferredId?: string | null) {
    const next = await invoke<ConfigProfile[]>("app_list_config_profiles");
    setProfiles(next);
    const nextId = preferredId === undefined ? selectedProfileId : preferredId;
    if (nextId && !next.some((profile) => profile.id === nextId)) {
      setSelectedProfileId(null);
    }
    return next;
  }

  async function loadProfileIntoForm(profile: ConfigProfile, confirmDiscard = true) {
    if (confirmDiscard && profileFormHasUnsavedChanges && !window.confirm("当前表单有未保存修改，确认载入其他 Profile？")) return;
    setProfileBusyId(profile.id);
    try {
      await chooseTool(profile.tool);
      setBaseUrl(profile.baseUrl);
      const referencedKey = profile.keyId == null ? null : state.keys.items.find((key) => key.id === profile.keyId);
      if (referencedKey) {
        await chooseKey(referencedKey.id);
      } else {
        const next = await invoke<AppStateData>("app_set_selected_key", { keyId: null });
        setState({ ...defaultState, ...next });
        setManualKey("");
      }
      setSelectedModel(profile.model || "");
      setSelectedReviewModel(profile.reviewModel || "");
      setRouteCodexEnabled(profile.localRoutingEnabled && profile.localRouteApps.includes("codex"));
      setRouteClaudeEnabled(profile.localRoutingEnabled && profile.localRouteApps.includes("claude"));
      setRouteOpenCodeEnabled(profile.localRoutingEnabled && profile.localRouteApps.includes("opencode"));
      setLocalRouteModelMap({ sonnet: profile.localRouteModelMap.sonnet || "", opus: profile.localRouteModelMap.opus || "", haiku: profile.localRouteModelMap.haiku || "" });
      setLocalRoutePreserveClaudeAuth(profile.localRoutePreserveClaudeAuth);
      setLocalRouteOnly(profile.localRouteOnly);
      setProfileNameDraft(profile.name);
      setSelectedProfileId(profile.id);
      if (profile.keyId != null && !referencedKey) setMessage("Profile 引用的远端 Key 当前不可用，请重新选择 Key");
      else if (profile.hasStoredKey) setMessage("已载入 Profile；手动 Key 已隐藏，一键应用可直接使用");
      else setMessage("已载入 Profile 到表单，尚未写入配置");
    } finally {
      setProfileBusyId(null);
    }
  }

  async function saveConfigProfile(profileId?: string) {
    const existing = profileId ? profiles.find((profile) => profile.id === profileId) : null;
    if (!currentProfileInput.name) {
      setError("请输入 Profile 名称");
      return;
    }
    if (existing && !window.confirm(`确认用当前表单覆盖 Profile「${existing.name}」？`)) return;
    setProfileBusyId(profileId || "new");
    setError(null);
    try {
      const saved = await invoke<ConfigProfile>("app_save_config_profile", {
        profileId: profileId || null,
        expectedUpdatedAt: existing?.updatedAt ?? null,
        profile: currentProfileInput,
      });
      await refreshProfiles(saved.id);
      setSelectedProfileId(saved.id);
      setProfileNameDraft(saved.name);
      if (saved.keyId == null && saved.hasStoredKey) {
        const next = await invoke<AppStateData>("app_set_selected_key", { keyId: null });
        setState({ ...defaultState, ...next });
        setManualKey("");
      }
      setMessage(existing ? "Profile 已覆盖保存" : "Profile 已创建");
    } catch (err) {
      setError(String(err));
    } finally {
      setProfileBusyId(null);
    }
  }

  async function deleteConfigProfile(profile: ConfigProfile) {
    if (!window.confirm(`确认删除 Profile「${profile.name}」？此操作不会修改已写入的工具配置。`)) return;
    setProfileBusyId(profile.id);
    setError(null);
    try {
      await invoke("app_delete_config_profile", { profileId: profile.id, expectedUpdatedAt: profile.updatedAt });
      await refreshProfiles(profile.id === selectedProfileId ? null : selectedProfileId);
      if (profile.id === selectedProfileId) {
        setSelectedProfileId(null);
        setProfileNameDraft("");
      }
      setMessage("Profile 已删除");
    } catch (err) {
      setError(String(err));
    } finally {
      setProfileBusyId(null);
    }
  }

  async function applyConfigProfile(profile: ConfigProfile) {
    if (profileFormHasUnsavedChanges && !window.confirm("当前表单有未保存修改，确认直接应用所选 Profile？")) return;
    setProfileBusyId(profile.id);
    try {
      const result = await run(() => invoke<ConfigTransactionResult>("app_apply_config_profile", { profileId: profile.id, apiKeyOverride: null }), "正在应用 Profile");
      if (result) {
        setPreview(result.artifacts);
        await loadProfileIntoForm(profile, false);
        setMessage(result.message);
      }
      await refreshConfigSnapshots();
      await refreshLocalRouteManifest();
    } finally {
      setProfileBusyId(null);
    }
  }

  async function refreshConfigSnapshots() {
    try {
      setConfigSnapshots(await invoke<ConfigSnapshotSummary[]>("app_list_config_snapshots"));
    } catch (err) {
      setError(String(err));
    }
  }

  async function restoreConfigSnapshot(snapshotId: string) {
    if (!window.confirm("确认回滚到这个配置版本？当前配置会先自动创建快照。")) return;
    setRestoringSnapshotId(snapshotId);
    try {
      const result = await run(() => invoke<ConfigTransactionResult>("app_restore_config_snapshot", { snapshotId }), "已恢复配置版本");
      if (result) {
        setPreview(result.artifacts);
        setMessage(result.message);
      }
      await refreshConfigSnapshots();
      await refreshLocalRouteManifest();
    } finally {
      setRestoringSnapshotId(null);
    }
  }

  async function showPreview() { const target = await run(prepareTarget, "已生成写入预览"); if (!target) return; setPreview(await invoke<[string, string][]>("app_copy_target_preview", { target })); }
  async function writeSwitch(): Promise<boolean> {
    try {
      const target = await run(prepareTarget, "已生成写入目标");
      const result = await run(() => invoke<ConfigTransactionResult>("app_write_switch", { target }), localRoutingEnabled ? "已写入配置并启动本地路由" : "已写入配置");
      setPreview(result.artifacts); setMessage(result.message);
      await refreshConfigSnapshots(); await refreshLocalRouteManifest();
      return true;
    } catch {
      return false;
    }
  }
  async function copyKey() { if (!effectiveKey) return; await navigator.clipboard.writeText(effectiveKey); setMessage("API Key 已复制"); }
  async function cleanupLocalRoute() { if (!window.confirm("确认清理本地路由接管？相关工具会恢复为清理后的配置。")) return; const result = await run(() => invoke<ConfigTransactionResult>("app_cleanup_local_route_takeover"), "已清理本地接管"); if (result) { setPreview(result.artifacts); setMessage(result.message); } await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }
  async function restoreLocalRouteBackups() { if (!window.confirm("确认用旧版备份覆盖当前工具配置？")) return; const result = await run(() => invoke<ConfigTransactionResult>("app_restore_local_route_backups"), "已从备份恢复配置"); if (result) { setPreview(result.artifacts); setMessage(result.message); } await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }
  function updateLocalRouteModel(role: string, value: string) { setLocalRouteModelMap((current) => ({ ...current, [role]: value })); }
  function fillLocalRouteModels() { if (!modelChoice) return; setLocalRouteModelMap({ sonnet: modelChoice, opus: modelChoice, haiku: modelChoice }); }

  useEffect(() => { void (async () => {
    try {
      const [nextState, nextTools, nextEndpoint, nextManifest, nextStatuses, nextPrefs, nextSnapshots, nextProfiles] = await Promise.all([
        invoke<AppStateData>("app_get_state"),
        invoke<ToolProfile[]>("app_get_tools"),
        invoke<EndpointProbeSummary>("app_get_endpoint").catch(() => null),
        invoke<LocalRouteManifest>("app_get_local_route_manifest").catch(() => null),
        invoke<LocalRouteStatus[]>("app_get_local_route_statuses").catch(() => []),
        invoke<AppPreferences>("app_get_preferences").catch(() => ({ onboardingCompleted: false, onboardingStep: 0, onboardingMode: null, dismissedAlertIds: [] })),
        invoke<ConfigSnapshotSummary[]>("app_list_config_snapshots").catch(() => []),
        invoke<ConfigProfile[]>("app_list_config_profiles").catch(() => []),
      ]);
      const onboardingPending = !nextPrefs?.onboardingCompleted;
      const restoredPreferences = nextPrefs || { onboardingCompleted: false, onboardingStep: 0, onboardingMode: null, dismissedAlertIds: [] };
      const restoredMode: OnboardingMode = restoredPreferences.onboardingMode === "official" || restoredPreferences.onboardingMode === "ai8888"
        ? restoredPreferences.onboardingMode
        : restoredPreferences.onboardingStep > 0 && Boolean(nextState.session) ? "ai8888" : "official";
      setPreferences(restoredPreferences);
      setWizardModeDraft(restoredMode);
      setShowWizard(onboardingPending);
      if (onboardingPending) setActiveView("switch");
      setConfigSnapshots(nextSnapshots);
      setProfiles(nextProfiles);
      setTools(nextTools);
      if (nextEndpoint) {
        setEndpointProbe(nextEndpoint);
        setBaseUrl(nextEndpoint.selectedBaseUrl);
        setMessage(`已选择可用端点 ${nextEndpoint.selectedDomain}`);
      }
      setLocalRouteManifest(nextManifest);
      setLocalRouteStatuses(nextStatuses);

      if (nextState.session) {
        try {
          // Account info is required; subscription expiry alone should not force re-login.
          const remote = await invoke<AppStateData>("app_load_remote_state");
          const preferredGroupId = recommendedSubscriptionGroupId(remote.subscriptions);
          setNewKeyGroupId(preferredGroupId?.toString() ?? remote.keys.items[0]?.group?.id?.toString() ?? "");
          if (onboardingPending && restoredMode === "ai8888" && restoredPreferences.onboardingStep > 0) {
            const preferredKey = preferredCodexKey(remote.keys.items, remote.groups, remote.subscriptions, remote.subscriptionProgress);
            if (preferredKey) {
              const selected = await invoke<AppStateData>("app_set_selected_key", { keyId: preferredKey.id });
              setState({ ...defaultState, ...selected });
              setManualKey(preferredKey.key ?? "");
            } else {
              setState({ ...defaultState, ...remote });
            }
          } else {
            setState({ ...defaultState, ...remote });
          }
          setAi8888DataReady(true);
          setError(null);
        } catch (err) {
          const text = formatAuthError(err);
          setState(isReloginError(text) ? { ...defaultState } : { ...defaultState, ...nextState });
          setAi8888DataReady(false);
          setError(text);
          setMessage(isReloginError(text) ? "请重新登录" : "操作失败");
        }
      } else {
        setState({ ...defaultState, ...nextState });
        setAi8888DataReady(false);
        const preferredGroupId = recommendedSubscriptionGroupId(nextState.subscriptions);
        setNewKeyGroupId(preferredGroupId?.toString() ?? nextState.keys.items[0]?.group?.id?.toString() ?? "");
      }
    } catch (err) {
      setError(formatAuthError(err));
    }
  })(); }, []);
  useEffect(() => { setSelectedModel((current) => current && models.some((item) => item.id === current) ? current : (models[0]?.id ?? current)); }, [models]);
  useEffect(() => { if (selectedKey) setEditKeyGroupId((current) => ({ ...current, [selectedKey.id]: keyGroupId(selectedKey)?.toString() ?? "" })); }, [selectedKey]);
  useEffect(() => {
    if (!showWizard || onboardingStep !== 1) {
      wizardAutoAdvanceRef.current = false;
      return;
    }
    if (wizardSaving || busy || codexAuth.busy || error || modelError) return;
    const officialReady = onboardingMode === "official" && Boolean(codexAuth.status?.authenticated) && !codexAuth.status?.loginRunning;
    const aiReady = onboardingMode === "ai8888" && isAuthenticated && ai8888DataReady && hasAi8888Funding;
    if (!officialReady && !aiReady) return;
    if (wizardAutoAdvanceRef.current) return;
    wizardAutoAdvanceRef.current = true;
    void advanceOnboarding();
  }, [showWizard, onboardingStep, onboardingMode, wizardSaving, busy, codexAuth.busy, codexAuth.status?.authenticated, codexAuth.status?.loginRunning, isAuthenticated, ai8888DataReady, hasAi8888Funding, error, modelError]);
  useEffect(() => {
    if (!showWizard || activeView !== "switch" || onboardingMode !== "ai8888" || !isAuthenticated || onboardingStep !== 2 || wizardModelAttempted || wizardModelRequestRef.current || testingModels || !effectiveKey || !baseUrl.trim()) return;
    wizardModelRequestRef.current = true;
    void testModels();
  }, [showWizard, activeView, onboardingMode, isAuthenticated, onboardingStep, wizardModelAttempted, testingModels, effectiveKey, baseUrl]);
  useEffect(() => {
    window.scrollTo({ top: 0, left: 0, behavior: "auto" });
    try { window.localStorage.setItem("ai8888-switch.active-view", activeView); } catch { /* Storage is optional. */ }
  }, [activeView]);

  return (
    <main className={`appFrame ${showWizard ? "wizardMode" : ""}`}>
      <aside className="appSidebar">
        <div className="appBrand"><span className="brandMark">A8</span><div><strong>AI8888 Switch</strong><small>AI 工具配置中心</small></div></div>
        <nav className="primaryNav" aria-label="主导航">
          {mainViews.map((item) => <button key={item.id} type="button" className={activeView === item.id ? "active" : ""} onClick={() => setActiveView(item.id)} disabled={showWizard}><strong>{item.label}</strong><small>{item.description}</small></button>)}
          <button type="button" onClick={() => void openCodexSessions()} disabled={showWizard}><strong>会话管理</strong><small>浏览并恢复跨工具会话</small></button>
        </nav>
        <div className="sidebarAccount">
          <div className="accountState"><span className={`dot ${isAuthenticated ? "ok" : ""}`} /><div><strong>{isAuthenticated ? state.account?.username || state.account?.email || "AI8888 已登录" : "AI8888 未登录"}</strong><small>{isAuthenticated ? `余额 ${money(state.account?.balance ?? 0)}` : "OpenAI 用户仍可使用本地功能"}</small></div></div>
          <div className="sidebarActions"><button className="ghost mini" onClick={openPurchase} disabled={showWizard}>充值续费</button>{isAuthenticated && <button className="ghost mini" onClick={() => void logout()} disabled={busy}>退出</button>}</div>
        </div>
      </aside>

      <div className="appContent">
        <header className="pageHeader">
          <div><p className="eyebrow">AI8888 Switch</p><h1>{activeViewMeta.label}</h1><p>{activeViewMeta.description}</p></div>
          <div className="pageStatus"><span className={`dot ${error || modelError ? "" : "ok"}`} /><div><strong>{busy ? "处理中" : message}</strong><small>{selectedProfile?.name ? `当前方案：${selectedProfile.name}` : selectedTool?.displayName || "本地工作台"}</small></div></div>
        </header>
        {error && <div className="alert">{error}</div>}{!showWizard && modelError && <div className="alert">{modelError}</div>}

      {!showWizard && activeView === "connections" && isAuthenticated && accountAlerts.length > 0 && <section className="alertStack">{accountAlerts.slice(0, 4).map((alert) => <article className={"alertCard " + alert.level} key={alert.id}><div><strong>{alert.title}</strong><p>{alert.detail}</p></div><div className="alertActions">{alert.action === "purchase" && <button className="secondary mini" onClick={() => void openPurchase()}>{"\u5145\u503c\u7eed\u8d39"}</button>}{alert.action === "refresh" && <button className="ghost mini" onClick={() => { void refreshRemote(); }} disabled={busy}>{"\u5237\u65b0\u7528\u91cf"}</button>}<button className="ghost mini" onClick={() => void dismissAlert(alert.id)}>{"\u5ffd\u7565"}</button></div></article>)}</section>}

      {activeView === "switch" && showWizard && (
        <section className="panel wizardPanel">
          <div className="wizardHeader">
            <div><p className="eyebrow">首次设置</p><h2>{wizardDisplayMode === "official" ? "登录一次，Codex 就能用" : "不用判断套餐还是余额"}</h2><p className="muted">{wizardDisplayMode === "official" ? "使用自己的 OpenAI 账户；也可以选择 AI8888，由系统自动判断套餐和余额。" : "系统会自动检查你买的是套餐还是余额；有可用套餐先用，没有才使用余额。"}</p></div>
            <button className="ghost mini" onClick={() => void deferOnboarding()} disabled={wizardSaving || busy || codexAuth.busy}>稍后设置</button>
          </div>
          <div className="wizardSteps" aria-label="首次设置进度">
            {onboardingStepLabels.map((label, index) => <div className={`wizardStep ${index < onboardingStep ? "complete" : index === onboardingStep ? "active" : ""}`} key={label}><span>{index < onboardingStep ? "✓" : index + 1}</span><small>{label}</small></div>)}
          </div>
          <div className="wizardBody">
            {onboardingStep === 0 && (
              <div className="wizardTask">
                <h3>你想用哪种账户？</h3>
                <p className="muted">只需要选择登录哪种账户，套餐、余额、Key 和线路都由系统自动处理。</p>
                <div className="wizardOptions">
                  <button type="button" className={`wizardOption ${wizardModeDraft === "official" ? "active" : ""}`} onClick={() => setWizardModeDraft("official")} disabled={wizardSaving || busy}><span><strong>OpenAI 官方账户</strong><small>使用自己的 ChatGPT / OpenAI 登录</small></span><span className="wizardOptionState">{wizardModeDraft === "official" ? "已选择" : "选择"}</span></button>
                  <button type="button" className={`wizardOption ${wizardModeDraft === "ai8888" ? "active" : ""}`} onClick={() => setWizardModeDraft("ai8888")} disabled={wizardSaving || busy}><span><strong>AI8888 账户</strong><small>登录购买服务的账户，套餐和余额都支持</small></span><span className="wizardOptionState">{wizardModeDraft === "ai8888" ? "已选择" : "选择"}</span></button>
                </div>
              </div>
            )}
            {onboardingStep === 1 && (
              <div className="wizardTask">
                {onboardingMode === "official" ? <>
                  <h3>登录 OpenAI 官方账户</h3>
                  <p className="muted">登录完成后会自动进入下一步；最后确认时再切换 Codex，不需要填写 Key 或线路。</p>
                  <div className="wizardChecks">
                    <div className="wizardCheck"><span className={`dot ${codexAuth.status?.cliAvailable ? "ok" : ""}`} /><div><strong>Codex CLI</strong><small>{codexAuth.status == null ? "正在检测" : codexAuth.status.cliAvailable ? `已找到 ${codexAuth.status.cliVersion || "Codex CLI"}` : "未找到 Codex CLI"}</small></div><button className="ghost mini" onClick={() => void codexAuth.refresh()} disabled={codexAuth.busy || codexAuth.status?.loginRunning}>{codexAuth.status?.cliAvailable ? "重新检测" : "检测"}</button></div>
                    <div className="wizardCheck"><span className={`dot ${codexAuth.status?.authenticated ? "ok" : ""}`} /><div><strong>OpenAI 账户</strong><small>{codexAuth.status?.authenticated ? "已登录，可以继续" : codexAuth.status?.loginRunning ? codexAuth.status.loginMode === "device" ? "请在打开的官方页面输入下方设备码" : "请在浏览器中完成授权" : codexAuth.status?.loginSucceeded === false ? codexAuth.status.loginMessage || "登录未完成，请重试" : "尚未登录"}</small></div></div>
                  </div>
                  {codexAuth.error && <div className="wizardRetry"><span>{codexAuth.error}</span><button className="ghost mini" onClick={() => void codexAuth.refresh()}>重试检测</button></div>}
                  {codexAuth.status?.loginOutput.length ? <details className="codexLoginOutput" open={codexAuth.status.loginRunning && codexAuth.status.loginMode === "device"}><summary>{codexAuth.status.loginRunning && codexAuth.status.loginMode === "device" ? "按下方设备码完成登录" : "查看登录信息"}</summary><div className="codexLoginOutputHead"><span>设备码/CLI 输出</span><button className="ghost mini" onClick={() => void navigator.clipboard.writeText(codexAuth.status?.loginOutput.join("\n") || "")}>复制</button></div><pre>{codexAuth.status.loginOutput.join("\n")}</pre></details> : null}
                  {!codexAuth.status?.authenticated && !codexAuth.status?.loginRunning && <div className="actions"><button onClick={() => void codexAuth.startLogin("browser", false)} disabled={codexAuth.busy || !codexAuth.status?.cliAvailable}>使用 ChatGPT 登录</button><button className="secondary" onClick={() => void startWizardDeviceLogin()} disabled={codexAuth.busy || !codexAuth.status?.cliAvailable}>设备码登录</button></div>}
                  {codexAuth.status?.loginRunning && <div className="actions"><button className="secondary" onClick={() => void codexAuth.openDevicePage()} disabled={codexAuth.busy || codexAuth.status.loginMode !== "device"}>打开官方认证页面</button><button className="ghost" onClick={() => void codexAuth.cancelLogin()} disabled={codexAuth.busy}>取消登录</button></div>}
                </> : <>
                  <h3>登录 AI8888 账户</h3>
                  {!isAuthenticated ? <>
                    <p className="muted">登录后会自动检查套餐和余额；有可用套餐时一定优先使用套餐。</p>
                    <form className="inlineForm loginForm wizardLoginForm" onSubmit={(event) => { event.preventDefault(); void login(); }}><input value={email} onChange={(event) => setEmail(event.target.value)} type="email" autoComplete="email" placeholder="邮箱" /><input value={password} onChange={(event) => setPassword(event.target.value)} type="password" autoComplete="current-password" placeholder="密码" /><button type="submit" disabled={busy || !email || !password}>登录</button></form>
                  </> : <>
                    <p className="muted">账户已登录，正在自动准备可用连接，你不用挑选 Key 或线路。</p>
                    <div className="wizardChecks"><div className="wizardCheck"><span className="dot ok" /><div><strong>账户</strong><small>{state.account?.email || state.account?.username || "AI8888 已登录"}</small></div></div><div className="wizardCheck"><span className={`dot ${hasAi8888Funding ? "ok" : ""}`} /><div><strong>使用来源</strong><small>{!ai8888DataReady ? "正在检查套餐和余额" : recommendedSubscriptionGroup != null ? `已找到「${recommendedSubscriptionName}」${recommendedSubscriptionExpiry}，将优先使用` : Number(state.account?.balance ?? 0) > 0 ? `未找到可用套餐，将使用账户余额 ${money(state.account?.balance ?? 0)}` : "没有可用套餐或余额"}</small></div></div></div>
                    {!ai8888DataReady && <div className="actions"><button className="secondary" onClick={() => void refreshRemote()} disabled={busy}>重新读取账户</button></div>}
                    {ai8888DataReady && !hasAi8888Funding && <div className="actions"><button className="secondary" onClick={openPurchase}>购买套餐或充值</button><button className="ghost" onClick={() => void openDailyReset()}>重置日卡额度</button></div>}
                  </>}
                </>}
              </div>
            )}
            {onboardingStep === 2 && (
              <div className="wizardTask">
                <h3>{onboardingMode === "official" ? "官方账户已连接" : "已自动准备好"}</h3>
                {onboardingMode === "official" ? <><p className="muted">下一步会把 Codex 切换到 OpenAI 官方账户。不会改动你的官方登录凭据。</p><div className="wizardSummary"><div><span>账户</span><strong>OpenAI 官方</strong></div><div><span>工具</span><strong>Codex</strong></div></div></> : <>
                  <p className="muted">账户和可用额度已经检查完成，不需要挑选 Key 或线路。</p>
                  <div className="wizardSummary"><div><span>使用来源</span><strong>{wizardFundingSource === "subscription" ? `套餐「${wizardSubscriptionName}」` : wizardFundingSource === "balance" ? `账户余额 ${money(state.account?.balance ?? 0)}` : "待重新准备"}</strong></div><div><span>连接状态</span><strong>{wizardSelectedKey ? "已自动准备" : "待重新准备"}</strong></div></div>
                  {testingModels && <div className="wizardLoading"><span className="dot ok" /><strong>正在读取可用模型…</strong></div>}
                  {!testingModels && models.length > 0 && <label className="wizardModelSelect">主模型（一般无需修改）<select value={selectedModel} onChange={(event) => setSelectedModel(event.target.value)}>{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>}
                  {!testingModels && wizardModelAttempted && models.length === 0 && !modelError && <div className="wizardLoading"><span className="dot ok" /><div><strong>使用默认模型</strong><small>服务端没有返回模型列表，仍可继续。</small></div></div>}
                  {modelError && <div className="wizardRetry"><span>连接验证失败，请重试后再继续。</span><button className="ghost mini" onClick={retryWizardModels}>重试</button></div>}
                </>}
              </div>
            )}
            {onboardingStep === 3 && (
              <div className="wizardTask">
                <h3>确认后立即应用</h3>
                <p className="muted">只需点击一次，成功后会自动创建可回滚快照。</p>
                <div className="wizardSummary">
                  <div><span>账户</span><strong>{onboardingMode === "official" ? "OpenAI 官方" : "AI8888"}</strong></div>
                  <div><span>使用来源</span><strong>{onboardingMode === "official" ? "OpenAI 官方账户" : wizardFundingSource === "subscription" ? `套餐「${wizardSubscriptionName}」优先` : wizardFundingSource === "balance" ? `账户余额 ${money(state.account?.balance ?? 0)}` : "未准备"}</strong></div>
                  <div><span>工具</span><strong>Codex</strong></div>
                  <div><span>模型</span><strong>{modelChoice || "默认模型"}</strong></div>
                </div>
              </div>
            )}
          </div>
          <div className="wizardActions">
            <div>{onboardingStep > 0 && <button className="ghost" onClick={() => void goBackOnboarding()} disabled={wizardSaving || busy || codexAuth.busy || probingEndpoint || testingModels}>上一步</button>}</div>
            {onboardingStep < 3 ? <button onClick={() => void advanceOnboarding()} disabled={wizardSaving || busy || codexAuth.busy || probingEndpoint || testingModels || (onboardingStep === 1 && (onboardingMode === "official" ? !codexAuth.status?.authenticated : !isAuthenticated || !ai8888DataReady || !hasAi8888Funding))}>{wizardSaving || probingEndpoint || testingModels ? "自动检查中…" : onboardingStep === 0 ? "选择并继续" : onboardingStep === 1 ? (onboardingMode === "official" ? "已登录，继续" : "自动检查并继续") : "确认这些设置"}</button> : <button onClick={() => void finishOnboarding()} disabled={wizardSaving || busy || codexAuth.busy || !canWrite}>{wizardSaving || busy || codexAuth.busy ? "正在应用…" : "确认并开始使用 Codex"}</button>}
          </div>
        </section>
      )}

      {!showWizard && activeView === "connections" && <>
      <CodexOfficialAccount controller={codexAuth} canActivateAi8888={Boolean(effectiveKey)} onActivateAi8888={activateAi8888ForCodex} />

      {!isAuthenticated ? <Ai8888LoginPanel email={email} password={password} setEmail={setEmail} setPassword={setPassword} busy={busy} onLogin={login} onOpenPurchase={openPurchase} /> : <section className="connectionSummary"><div><span className="dot ok" /><div><strong>{state.account?.username || state.account?.email || "AI8888 账户"}</strong><small>余额 {money(state.account?.balance ?? 0)} · 并发 {state.account?.concurrency ?? "-"}</small></div></div><div className="actions"><button className="secondary" onClick={() => { void refreshRemote(); }} disabled={busy}>刷新账户</button><button className="ghost" onClick={openPurchase}>充值续费</button><button className="ghost" onClick={openDailyReset}>日卡重置</button><button className="ghost" onClick={() => void logout()} disabled={busy}>退出</button></div></section>}
      <div className="utilityBar"><span>账户服务</span><div className="actions"><button className="ghost" onClick={openModelStatus}>模型监控</button><button className="ghost" onClick={openRadar}>智商雷达</button><button className="ghost" onClick={() => void openCodexSessions()}>会话管理</button></div></div>

      {isAuthenticated && <section className="grid two accountGrid">
        <div className="panel"><div className="panelHead"><h2>订阅</h2><div className="actions"><button className="secondary mini" onClick={() => void openDailyReset()}>日卡重置</button><span className="badge">可用 {state.subscriptions.filter(isActiveSubscription).length} / 总计 {state.subscriptions.length}</span></div></div><div className="list">{state.subscriptions.length === 0 && <p className="muted">暂无订阅</p>}{state.subscriptions.map((sub) => { const progress = subscriptionProgressInfo(sub, state.subscriptionProgress); const daily = usageWindow(sub, progress, "daily"); const weekly = usageWindow(sub, progress, "weekly"); const monthly = usageWindow(sub, progress, "monthly"); return <article className="row subscriptionRow" key={sub.id}><div><strong>{progress?.groupName || sub.groupName || sub.group?.name || `订阅 #${sub.id}`}</strong><small>{sub.status} - 到期 {safeDate(progress?.expiresAt ?? sub.expiresAt)}</small><small>{quotaLine(sub, progress)}</small><div className="usageGrid"><span>日：已用 {moneyOrDash(daily.used)} / 限额 {moneyOrDash(daily.limit)} / 剩余 {moneyOrDash(daily.remaining)} / {percentLabel(daily.used, daily.limit, daily.percentage)}</span><span>周：已用 {moneyOrDash(weekly.used)} / 限额 {moneyOrDash(weekly.limit)} / 剩余 {moneyOrDash(weekly.remaining)} / {percentLabel(weekly.used, weekly.limit, weekly.percentage)}</span><span>月：已用 {moneyOrDash(monthly.used)} / 限额 {moneyOrDash(monthly.limit)} / 剩余 {moneyOrDash(monthly.remaining)} / {percentLabel(monthly.used, monthly.limit, monthly.percentage)}</span></div></div><span>{isActiveSubscription(sub) ? "有效" : "无效"}</span></article>; })}</div></div>
        <div className="panel"><div className="panelHead"><h2>API Key</h2><span className="badge">{state.keys.total}</span></div><div className="inlineForm"><input value={newKeyName} onChange={(e) => setNewKeyName(e.target.value)} placeholder="Key 名称" /><select value={newKeyGroupId} onChange={(e) => setNewKeyGroupId(e.target.value)}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}{group.platform ? ` - ${group.platform}` : ""}</option>)}</select><button onClick={() => void createKey()} disabled={busy || !newKeyName}>创建</button></div><div className="list keys">{state.keys.items.length === 0 && <p className="muted">暂无 Key。请先创建或同步 API Key。</p>}{state.keys.items.map((item) => { const resolvedGroup = keyGroup(item, state.groups); const groupValue = editKeyGroupId[item.id] ?? (keyGroupId(item)?.toString() ?? ""); return <article className={"row selectable " + (selectedKey?.id === item.id ? "selected" : "")} key={item.id} onClick={() => chooseKey(item.id)}><div><strong>{item.name || `Key #${item.id}`}</strong><small>{item.status || "unknown"} - {resolvedGroup?.name || "未分组"} - {maskKey(item.key)}</small></div><div className="actions"><select value={groupValue} onChange={(e) => { e.stopPropagation(); setEditKeyGroupId((cur) => ({ ...cur, [item.id]: e.target.value })); void updateKeyGroup(item.id, e.target.value); }} onClick={(e) => e.stopPropagation()}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}</select><button className="danger mini" onClick={(e) => { e.stopPropagation(); void deleteKey(item.id); }}>删除</button></div></article>; })}</div></div>
      </section>}
      </>}

      {activeView === "switch" && !showWizard && <>
      {!isAuthenticated && <div className="emptyBanner"><strong>AI8888 账户尚未登录</strong><span>你仍可管理本地工作区和 OpenAI 官方账户；登录 AI8888 后可同步 Key 并写入 AI8888 配置。</span><button onClick={() => setActiveView("connections")}>前往账户与连接</button></div>}
      <section className="panel profilePanel">
        <div className="panelHead"><h2>配置方案</h2><span className="badge">{profiles.length}</span></div>
        <div className="profileComposer">
          <input value={profileNameDraft} onChange={(event) => setProfileNameDraft(event.target.value)} placeholder="方案名称，例如：工作 / 测试 / 备用" maxLength={80} />
          <button className="secondary" onClick={() => { void saveConfigProfile(); }} disabled={busy || profileBusyId !== null || !profileNameDraft.trim()}>保存为新方案</button>
          {selectedProfile && <button onClick={() => { void saveConfigProfile(selectedProfile.id); }} disabled={busy || profileBusyId !== null || !profileDirty}>{profileBusyId === selectedProfile.id ? "保存中" : "覆盖保存 / 重命名"}</button>}
        </div>
        <p className="muted">方案保存端点、Key、模型、目标工具和路由规则。载入只修改表单，一键应用会立即执行配置事务。</p>
        <div className="list profileList">
          {profiles.length === 0 && <p className="muted">尚无配置方案。填写下方配置后保存即可复用。</p>}
          {profiles.map((profile) => {
            const referencedKey = profile.keyId == null ? null : state.keys.items.find((key) => key.id === profile.keyId);
            const keyText = referencedKey?.name || (profile.keyId != null ? `Key #${profile.keyId}（当前不可用）` : profile.keyHint || "已保存手动 Key");
            const routeText = profile.localRoutingEnabled ? `路由：${profile.localRouteApps.join(" / ") || "未选择"}` : "直连";
            return <article className={"row profileRow " + (selectedProfileId === profile.id ? "selected" : "")} key={profile.id}><div><strong>{profile.name}</strong><small>{profile.tool} · {profile.baseUrl} · {profile.model || "默认模型"} · 审核 {profile.reviewModel || "跟随主模型"}</small><small>{keyText} · {routeText}</small></div><div className="actions"><button className="ghost mini" onClick={() => { void loadProfileIntoForm(profile); }} disabled={busy || profileBusyId !== null}>载入表单</button><button className="secondary mini" onClick={() => { void applyConfigProfile(profile); }} disabled={busy || profileBusyId !== null}>{profileBusyId === profile.id ? "处理中" : "一键应用"}</button><button className="danger mini" onClick={() => { void deleteConfigProfile(profile); }} disabled={busy || profileBusyId !== null}>删除</button></div></article>;
          })}
        </div>
      </section>
      </>}

      {!showWizard && (activeView === "workspace" || activeView === "usage" || activeView === "settings") && <WorkspaceCenter
        key={activeView}
        section={activeView === "workspace" ? "workspace" : activeView}
        profiles={profiles.map((profile) => ({ id: profile.id, name: profile.name }))}
        selectedProfileId={selectedProfileId}
        onProfileApplied={async (profileId) => {
          const profile = profiles.find((item) => item.id === profileId);
          if (profile) await applyConfigProfile(profile);
        }}
      />}

      {activeView === "switch" && !showWizard && <>
      <section className="panel switchPanel">
        <div className="panelHead"><h2>写入配置</h2><span className="badge">{selectedTool?.displayName ?? state.selectedTool}</span></div>
        <div className="switchGrid">
          <label>目标工具<select value={state.selectedTool} onChange={(e) => chooseTool(e.target.value)}>{tools.map((tool) => <option key={tool.tool} value={tool.tool}>{tool.displayName}</option>)}</select></label>
          <label>接口 Base URL<div className="inputAction"><input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /><button className="secondary" onClick={probeBestEndpoint} disabled={probingEndpoint || busy}>{probingEndpoint ? "检测中" : "检测最优端点"}</button></div></label>
          {endpointProbe && <div className="wide endpointProbeResults">{endpointProbe.results.map((result) => <span className={result.selected ? "selectedEndpoint" : ""} key={result.domain}>{result.selected ? "已选 " : ""}{endpointProbeText(result)}</span>)}</div>}
          <label className="wide">API Key<input value={manualKey} onChange={(e) => setManualKey(e.target.value)} placeholder={selectedKey?.key ? "使用选中的 Key" : "sk-..."} /></label>
          <label className="wide">模型选择（可选）<button className="secondary" onClick={() => void testModels()} disabled={testingModels || !effectiveKey || !baseUrl.trim()}>获取模型列表</button><select value={selectedModel} onChange={(e) => setSelectedModel(e.target.value)} disabled={testingModels}>{!selectedModel && models.length === 0 ? <option value="">可不选</option> : null}{selectedModel && !models.some((model) => model.id === selectedModel) ? <option value={selectedModel}>{selectedModel} - Profile</option> : null}{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>
          {state.selectedTool === "codex" && <label className="wide">自动审核模型（可选，留空跟随主模型）<select value={selectedReviewModel} onChange={(e) => setSelectedReviewModel(e.target.value)} disabled={testingModels}><option value="">跟随主模型</option>{selectedReviewModel && !models.some((model) => model.id === selectedReviewModel) ? <option value={selectedReviewModel}>{selectedReviewModel} - Profile</option> : null}{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>}
          <details className="advancedSettings wide">
            <summary><span>本地路由与高级选项</span><small>{localRoutingEnabled ? `已启用 ${localRouteApps.length} 个工具` : "未启用"}</small></summary>
            <div className="advancedSettingsBody">
              <label className="checkboxLine"><input type="checkbox" checked={routeCodexEnabled} onChange={(e) => setRouteCodexEnabled(e.target.checked)} /> 启用 Codex 本地路由</label>
              <label className="checkboxLine"><input type="checkbox" checked={routeClaudeEnabled} onChange={(e) => setRouteClaudeEnabled(e.target.checked)} /> 启用 Claude 本地路由</label>
              <label className="checkboxLine"><input type="checkbox" checked={routeOpenCodeEnabled} onChange={(e) => setRouteOpenCodeEnabled(e.target.checked)} /> 启用 OpenCode 本地路由</label>
              {routeClaudeEnabled && <div className="routeBox"><div className="routeTitle">Claude 路由模型映射</div><div className="modelMapGrid"><label>Sonnet<input value={localRouteModelMap.sonnet} onChange={(e) => updateLocalRouteModel("sonnet", e.target.value)} /></label><label>Opus<input value={localRouteModelMap.opus} onChange={(e) => updateLocalRouteModel("opus", e.target.value)} /></label><label>Haiku<input value={localRouteModelMap.haiku} onChange={(e) => updateLocalRouteModel("haiku", e.target.value)} /></label></div><div className="actions"><button className="secondary" onClick={fillLocalRouteModels} disabled={!modelChoice}>用当前模型填充</button><label className="checkboxLine"><input type="checkbox" checked={localRoutePreserveClaudeAuth} onChange={(e) => setLocalRoutePreserveClaudeAuth(e.target.checked)} /> 保留 Claude 现有认证</label></div></div>}
              {localRoutingEnabled && <label className="checkboxLine"><input type="checkbox" checked={localRouteOnly} onChange={(e) => setLocalRouteOnly(e.target.checked)} /> 只接管路由，不写模型</label>}
              <div className="actions maintenanceActions"><button className="ghost" onClick={copyKey} disabled={!effectiveKey}>复制 Key</button><button className="danger" onClick={cleanupLocalRoute} disabled={busy}>清理本地路由</button><button className="danger" onClick={restoreLocalRouteBackups} disabled={busy}>恢复旧版备份</button></div>
            </div>
          </details>
        </div>
        {selectedTool && <p className="muted">将写入：{selectedTool.configPath}。{selectedTool.notes}</p>}
        <div className="actions primaryActions"><button onClick={writeSwitch} disabled={!canWrite || busy}>应用配置</button><button className="secondary" onClick={showPreview} disabled={busy || !effectiveKey}>预览变更</button></div>
      </section>

      <section className="panel configHistory">
        <div className="panelHead"><h2>配置历史</h2><span className="badge">{configSnapshots.length}</span></div>
        <div className="list">
          {configSnapshots.length === 0 && <p className="muted">首次成功写入配置后会生成可回滚快照。</p>}
          {configSnapshots.map((snapshot) => <article className="row" key={snapshot.id}><div><strong>{snapshot.label}</strong><small>{safeTimestamp(snapshot.createdAt)} · {snapshot.files.length} 个文件</small></div><button className="danger mini" onClick={() => { void restoreConfigSnapshot(snapshot.id); }} disabled={busy || restoringSnapshotId !== null}>{restoringSnapshotId === snapshot.id ? "恢复中" : "回滚"}</button></article>)}
        </div>
      </section>

      {preview.length > 0 && <section className="panel"><div className="panelHead"><h2>写入目标</h2></div><div className="list">{preview.map(([path, label]) => <article className="row" key={path}><div><strong>{label}</strong><small>{path}</small></div></article>)}</div></section>}
      {localRouteManifest && localRouteManifest.entries.length > 0 && <section className="panel routeManifest"><div className="panelHead"><h2>本地路由状态</h2></div>{localRouteManifest.entries.map((entry) => <div className="routeEntry" key={entry.app}><strong>{appLabel(entry.app)} - {entry.localBaseUrl}</strong><small>模型：{entry.model || "默认"}</small></div>)}{localRouteStatuses.map((status) => <div className={"routeEntry " + (status.detected ? "okEntry" : "")} key={status.app}><strong>{appLabel(status.app)}：{status.detected ? "已接管" : "未接管"}</strong><small>{status.detail}</small></div>)}</section>}
      </>}
      {!showWizard && activeView === "settings" && <footer className="appFooter">
        <div>v0.1.1 Copyright AI8888.SHOP 2026</div>
        <div className="footerActions">
          <button className="ghost mini" onClick={() => { void restartOnboarding(); }} disabled={wizardSaving || busy}>重新运行首次设置</button>
          <button className="ghost mini" onClick={() => { void checkUpdate(); }} disabled={checkingUpdate || installingUpdate}>{checkingUpdate ? "检查中" : "检查更新"}</button>
          {updateInfo?.updateAvailable && updateInfo.downloadUrl && <button className="secondary mini" onClick={() => { void installUpdate(); }} disabled={checkingUpdate || installingUpdate}>{installingUpdate ? "正在下载安装" : "下载并安装"}</button>}
          {(updateInfo?.downloadUrl || updateInfo?.releaseUrl) && <a href={updateInfo.downloadUrl || updateInfo.releaseUrl || "#"} target="_blank" rel="noreferrer">{updateInfo.updateAvailable ? (updateInfo.downloadUrl ? (updateInfo.downloadAccelerated ? "加速下载链接" : "直接下载链接") : "查看新版本") : "GitHub Releases"}</a>}
          {updateInfo?.updateAvailable && updateInfo?.releaseUrl && updateInfo?.downloadUrl && <a href={updateInfo.releaseUrl} target="_blank" rel="noreferrer">发布页</a>}
        </div>
        {updateProgress && installingUpdate && <div className="updateProgress"><div><strong>{updateProgress.message}</strong><small>{updateProgress.totalBytes > 0 ? `${bytesLabel(updateProgress.downloadedBytes)} / ${bytesLabel(updateProgress.totalBytes)} · ${updateProgress.percent.toFixed(1)}%` : "正在连接"}</small></div><progress max="100" value={updateProgress.percent} />{["preparing", "downloading", "fallback"].includes(updateProgress.status) && <button className="ghost mini" onClick={() => { void cancelUpdate(); }}>取消</button>}</div>}
        {updateInfo && <div className="muted">{updateInfo.updateAvailable ? `发现新版本 ${updateInfo.latestVersion}${updateInfo.downloadAccelerated ? "（已启用 GitHub 加速下载）" : updateInfo.mainlandChina ? "（大陆网络，未找到安装包资源）" : ""}` : updateInfo.error ? `更新检查失败：${updateInfo.error}` : `当前已是最新版本 ${updateInfo.currentVersion}`}</div>}
        <div className="credits">{"\u81f4\u8c22\u5f00\u6e90\u9879\u76ee\uff1a"}<a href="https://github.com/jlcodes99/cockpit-tools" target="_blank" rel="noreferrer">cockpit-tools</a><a href="https://github.com/jlcodes99/cc-switch" target="_blank" rel="noreferrer">cc-switch</a><a href="https://github.com/Wei-Shaw/sub2api" target="_blank" rel="noreferrer">sub2api</a></div>
      </footer>}
      </div>
    </main>
  );
}

const RootApp = new URLSearchParams(window.location.search).get("view") === "sessions" ? CodexSessionsApp : App;

createRoot(document.getElementById("root")!).render(<React.StrictMode><RootApp /></React.StrictMode>);


