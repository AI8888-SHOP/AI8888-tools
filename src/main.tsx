import React, { useCallback, useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";
import { buildAccountAlerts, isActiveSubscription, money, moneyOrDash, percentLabel, quotaLine, subscriptionProgressInfo, usageWindow, type GroupSummary, type SubscriptionProgress, type SubscriptionProgressInfo, type SubscriptionSummary } from "./subscription";
import CodexOfficialAccount from "./CodexOfficialAccount";
import WorkspaceCenter from "./WorkspaceCenter";

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
type AppPreferences = { onboardingCompleted: boolean; onboardingStep: number; dismissedAlertIds: string[] };
type CodexSessionMeta = { sessionId: string; title?: string | null; summary?: string | null; projectDir?: string | null; createdAt?: string | null; lastActiveAt?: string | null; modelProvider?: string | null; modelProviderKey?: string | null; sourcePath: string; resumeCommand: string; archived: boolean; modifiedAt: number };
type CodexSessionSearchHit = { session: CodexSessionMeta; matchedIn: string[]; snippet?: string | null };
type CodexSessionMessage = { role: string; content: string; timestamp?: string | null };
type CodexSessionVisibilityRepairOutcome = { sessionId: string; sourcePath: string; success: boolean; changed: boolean; error?: string | null };
type UnifiedSessionMeta = { source: string; sourceLabel: string; sessionId: string; title?: string | null; summary?: string | null; projectDir?: string | null; createdAt?: string | null; lastActiveAt?: string | null; model?: string | null; sourcePath: string; resumeCommand?: string | null; archived: boolean; modifiedAt: number; messageCount?: number | null };
type UnifiedSessionMessage = { role: string; content: string; timestamp?: string | null; messageType?: string | null };

const defaultState: AppStateData = { account: null, subscriptions: [], subscriptionProgress: [], groups: [], keys: { items: [], total: 0 }, selectedTool: "codex", selectedKeyId: null, loginWindowOpen: false };

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
function keyGroup(key: ApiKeySummary | null | undefined, groups: GroupSummary[]) { return key?.group ?? groups.find((group) => group.id === key?.groupId) ?? null; }

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

function AuthGate(props: { email: string; password: string; setEmail: (v: string) => void; setPassword: (v: string) => void; busy: boolean; message: string; error: string | null; onLogin: () => void; onOpenPurchase: () => void; onOpenRadar: () => void; onOpenModelStatus: () => void }) {
  return (
    <main className="shell authShell">
      <section className="hero authHero">
        <div>
          <p className="eyebrow">AI8888 Switch</p>
          <h1>选择账户并开始使用</h1>
          <p className="heroText">登录 AI8888 管理订阅与 Key，或直接连接 OpenAI 官方 Codex 账户。</p>
        </div>
        <div className="statusCard authCard">
          <div>
            <strong>AI8888 未登录</strong>
            <small>{props.message}</small>
            <div className="statusActions"><button className="ghost mini statusButton" onClick={props.onOpenPurchase}>充值续费</button><button className="ghost mini statusButton" onClick={props.onOpenRadar}>智商雷达</button><button className="ghost mini statusButton" onClick={props.onOpenModelStatus}>模型监控</button></div>
          </div>
        </div>
      </section>
      {props.error && <div className="alert">{props.error}</div>}
      <section className="panel authPanel">
        <div className="panelHead"><h2>登录</h2></div>
        <div className="inlineForm authForm">
          <input value={props.email} onChange={(e) => props.setEmail(e.target.value)} placeholder="邮箱" />
          <input value={props.password} onChange={(e) => props.setPassword(e.target.value)} type="password" placeholder="密码" />
          <button onClick={props.onLogin} disabled={props.busy || !props.email || !props.password}>登录</button>
        </div>
      </section>
      <CodexOfficialAccount standalone />
      <footer className="appFooter">v0.1.0 Copyright AI8888.SHOP 2026</footer>
    </main>
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
  const [preferences, setPreferences] = useState<AppPreferences>({ onboardingCompleted: true, onboardingStep: 0, dismissedAlertIds: [] });
  const [showWizard, setShowWizard] = useState(false);
  const [routeCodexEnabled, setRouteCodexEnabled] = useState(false);
  const [routeClaudeEnabled, setRouteClaudeEnabled] = useState(false);
  const [routeOpenCodeEnabled, setRouteOpenCodeEnabled] = useState(false);
  const [localRouteModelMap, setLocalRouteModelMap] = useState<Record<string, string>>({ sonnet: "", opus: "", haiku: "" });
  const [localRoutePreserveClaudeAuth, setLocalRoutePreserveClaudeAuth] = useState(false);
  const [localRouteOnly, setLocalRouteOnly] = useState(false);
  const [localRouteManifest, setLocalRouteManifest] = useState<LocalRouteManifest | null>(null);
  const [localRouteStatuses, setLocalRouteStatuses] = useState<LocalRouteStatus[]>([]);

  const selectedTool = useMemo(() => tools.find((tool) => tool.tool === state.selectedTool) ?? tools[0], [state.selectedTool, tools]);
  const selectedKey = useMemo(() => state.selectedKeyId == null ? null : (state.keys.items.find((key) => key.id === state.selectedKeyId) ?? null), [state.keys.items, state.selectedKeyId]);
  const selectedProfile = useMemo(() => profiles.find((profile) => profile.id === selectedProfileId) ?? null, [profiles, selectedProfileId]);
  const accountAlerts = useMemo(() => {
    const all = buildAccountAlerts({ balance: state.account?.balance, subscriptions: state.subscriptions, subscriptionProgress: state.subscriptionProgress });
    const dismissed = new Set(preferences.dismissedAlertIds || []);
    return all.filter((alert) => !dismissed.has(alert.id));
  }, [state.account?.balance, state.subscriptions, state.subscriptionProgress, preferences.dismissedAlertIds]);

  const isAuthenticated = Boolean(state.session);
  const effectiveKey = manualKey.trim() || selectedKey?.key || "";
  const modelChoice = selectedModel.trim();
  const reviewModelChoice = selectedReviewModel.trim();
  const canWrite = Boolean(state.session && effectiveKey && baseUrl.trim());
  const localRoutingEnabled = routeCodexEnabled || routeClaudeEnabled || routeOpenCodeEnabled;
  const localRouteApps = useMemo(() => {
    const apps: string[] = [];
    if (routeCodexEnabled) apps.push("codex");
    if (routeClaudeEnabled) apps.push("claude");
    if (routeOpenCodeEnabled) apps.push("opencode");
    return apps;
  }, [routeCodexEnabled, routeClaudeEnabled, routeOpenCodeEnabled]);
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
      } else {
        const next = await run(() => invoke<AppStateData>("app_load_remote_state"), "\u5df2\u5237\u65b0\u4f59\u989d\u3001\u8ba2\u9605\u4e0e Key");
        if (next) setState({ ...defaultState, ...next });
      }
    } catch (err) {
      const text = formatAuthError(err);
      if (isReloginError(text)) {
        setState({ ...defaultState });
        setManualKey("");
        setModels([]);
        setSelectedModel("");
        setSelectedReviewModel("");
        setModelError(null);
        setError(text);
        setMessage("\u8bf7\u91cd\u65b0\u767b\u5f55");
        return;
      }
      if (!silent) {
        setError(text);
        setMessage("\u64cd\u4f5c\u5931\u8d25");
      }
    }
  }, []);
  async function chooseTool(tool: string) { const next = await invoke<AppStateData>("app_set_selected_tool", { tool }); setState({ ...defaultState, ...next }); setModels([]); setSelectedModel(""); setSelectedReviewModel(""); setModelError(null); }
  async function chooseKey(keyId: number) { const next = await invoke<AppStateData>("app_set_selected_key", { keyId }); setState({ ...defaultState, ...next }); const selectedItem = next.keys.items.find((item) => item.id === keyId); setNewKeyGroupId(keyGroupId(selectedItem)?.toString() ?? ""); setManualKey(selectedItem?.key ?? ""); }
  async function updateKeyGroup(keyId: number, groupId: string) { await run(() => invoke("app_update_key_group", { keyId, groupId: groupId ? Number(groupId) : null }), "已更新 Key 分组"); await refreshLocalState(); }
  async function createKey() { const created = await run(() => invoke<ApiKeySummary>("app_create_key", { payload: { name: newKeyName, groupId: newKeyGroupId ? Number(newKeyGroupId) : null } }), "API Key 已创建"); if (created?.key) setManualKey(created.key); await refreshLocalState(); }
  async function deleteKey(keyId: number) { await run(() => invoke("app_delete_key", { keyId }), "API Key 已删除"); await refreshLocalState(); }
  async function login() { const next = await run(() => invoke<AppStateData>("app_login_with_password", { email, password }), "登录成功"); if (next) { setState({ ...defaultState, ...next }); setPassword(""); await refreshLocalRouteManifest(); if (!preferences.onboardingCompleted) setShowWizard(true); } }
  async function logout() { const next = await run(() => invoke<AppStateData>("app_logout"), "已退出登录"); if (next) { setState({ ...defaultState, ...next }); setManualKey(""); setModels([]); setSelectedModel(""); setSelectedReviewModel(""); setModelError(null); } }
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

  async function completeOnboarding() {
    try {
      const next = await invoke<AppPreferences>("app_complete_onboarding");
      setPreferences(next);
      setShowWizard(false);
      setMessage("\u9996\u6b21\u5f15\u5bfc\u5df2\u5b8c\u6210");
    } catch (err) {
      setError(String(err));
    }
  }

  async function saveOnboardingStep(step: number) {
    const next = { ...preferences, onboardingCompleted: false, onboardingStep: step, dismissedAlertIds: preferences.dismissedAlertIds || [] };
    setPreferences(next);
    try {
      const saved = await invoke<AppPreferences>("app_set_preferences", { preferences: next });
      setPreferences(saved);
    } catch (err) {
      setError(String(err));
    }
  }

  async function testModels() {
    setTestingModels(true); setModelError(null);
    try { const next = await invoke<ModelSummary[]>("app_fetch_models", { query: { baseUrl, apiKey: effectiveKey, isFullUrl: false, modelsUrlOverride: null, userAgent: null } }); setModels(next); setSelectedModel((current) => current && next.some((item) => item.id === current) ? current : (next[0]?.id ?? "")); setMessage(`已获取 ${next.length} 个模型`); }
    catch (err) { setModelError(String(err)); setModels([]); }
    finally { setTestingModels(false); }
  }

  async function probeBestEndpoint() {
    setProbingEndpoint(true); setError(null); setEndpointProbe(null);
    try {
      const summary = await invoke<EndpointProbeSummary>("app_probe_best_endpoint");
      setEndpointProbe(summary);
      setBaseUrl(summary.selectedBaseUrl);
      const picked = summary.results.find((item) => item.selected);
      const loss = picked ? `${Math.round(picked.packetLoss * 100)}%` : "-";
      const latency = picked?.averageLatencyMs == null ? "-" : `${Math.round(picked.averageLatencyMs)}ms`;
      setMessage(`已选择最优端点 ${summary.selectedDomain}（丢包 ${loss}，延迟 ${latency}）`);
    } catch (err) {
      const text = err instanceof Error ? err.message : String(err);
      setError(text);
      setMessage("端点检测失败");
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
  async function writeSwitch() { const target = await run(prepareTarget, "已生成写入目标"); if (!target) return; const result = await run(() => invoke<ConfigTransactionResult>("app_write_switch", { target }), localRoutingEnabled ? "已写入配置并启动本地路由" : "已写入配置"); if (result) { setPreview(result.artifacts); setMessage(result.message); } await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }
  async function copyKey() { if (!effectiveKey) return; await navigator.clipboard.writeText(effectiveKey); setMessage("API Key 已复制"); }
  async function cleanupLocalRoute() { const result = await run(() => invoke<ConfigTransactionResult>("app_cleanup_local_route_takeover"), "已清理本地接管"); if (result) { setPreview(result.artifacts); setMessage(result.message); } await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }
  async function restoreLocalRouteBackups() { const result = await run(() => invoke<ConfigTransactionResult>("app_restore_local_route_backups"), "已从备份恢复配置"); if (result) { setPreview(result.artifacts); setMessage(result.message); } await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }
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
        invoke<AppPreferences>("app_get_preferences").catch(() => ({ onboardingCompleted: false, onboardingStep: 0, dismissedAlertIds: [] })),
        invoke<ConfigSnapshotSummary[]>("app_list_config_snapshots").catch(() => []),
        invoke<ConfigProfile[]>("app_list_config_profiles").catch(() => []),
      ]);
      setPreferences(nextPrefs || { onboardingCompleted: false, onboardingStep: 0, dismissedAlertIds: [] });
      setShowWizard(!(nextPrefs?.onboardingCompleted));
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
          setState({ ...defaultState, ...remote });
          setNewKeyGroupId(remote.keys.items[0]?.group?.id?.toString() ?? "");
          setError(null);
        } catch (err) {
          const text = formatAuthError(err);
          setState({ ...defaultState });
          setError(text);
          setMessage(isReloginError(text) ? "请重新登录" : "操作失败");
        }
      } else {
        setState({ ...defaultState, ...nextState });
        setNewKeyGroupId(nextState.keys.items[0]?.group?.id?.toString() ?? "");
      }
    } catch (err) {
      setError(formatAuthError(err));
    }
  })(); }, []);
  useEffect(() => { setSelectedModel((current) => current && models.some((item) => item.id === current) ? current : (models[0]?.id ?? current)); }, [models]);
  useEffect(() => { if (selectedKey) setEditKeyGroupId((current) => ({ ...current, [selectedKey.id]: keyGroupId(selectedKey)?.toString() ?? "" })); }, [selectedKey]);

  if (!isAuthenticated) return <AuthGate email={email} password={password} setEmail={setEmail} setPassword={setPassword} busy={busy} message={message} error={error} onLogin={login} onOpenPurchase={openPurchase} onOpenRadar={openRadar} onOpenModelStatus={openModelStatus} />;

  return (
    <main className="shell">
      <section className="hero">
        <div><p className="eyebrow">AI8888 Switch</p><h1>切换 AI8888 API 配置</h1><p className="heroText">同步账户、订阅、分组与 API Key，一键写入 Codex、Claude、OpenCode 等工具。</p></div>
        <div className="statusCard"><span className="dot ok" /><div><strong>已登录</strong><small>{message}</small><small className="balanceLine">账户余额：{money(state.account?.balance ?? 0)} <button className="ghost mini inlineRefresh" onClick={() => { void refreshRemote(); }} disabled={busy}>刷新</button></small><div className="statusActions"><button className="ghost mini statusButton" onClick={openPurchase}>充值续费</button><button className="ghost mini statusButton" onClick={openRadar}>智商雷达</button><button className="ghost mini statusButton" onClick={openModelStatus}>模型监控</button><button className="ghost mini statusButton" onClick={logout} disabled={busy}>退出登录</button></div></div></div>
      </section>
      {error && <div className="alert">{error}</div>}{modelError && <div className="alert">{modelError}</div>}

      {accountAlerts.length > 0 && <section className="alertStack">{accountAlerts.slice(0, 4).map((alert) => <article className={"alertCard " + alert.level} key={alert.id}><div><strong>{alert.title}</strong><p>{alert.detail}</p></div><div className="alertActions">{alert.action === "purchase" && <button className="secondary mini" onClick={() => void openPurchase()}>{"\u5145\u503c\u7eed\u8d39"}</button>}{alert.action === "refresh" && <button className="ghost mini" onClick={() => { void refreshRemote(); }} disabled={busy}>{"\u5237\u65b0\u7528\u91cf"}</button>}<button className="ghost mini" onClick={() => void dismissAlert(alert.id)}>{"\u5ffd\u7565"}</button></div></article>)}</section>}

      {showWizard && isAuthenticated && (
        <section className="panel wizardPanel">
          <div className="panelHead">
            <h2>{"首次引导"}</h2>
            <span className="badge">{"步骤 "}{(preferences.onboardingStep || 0) + 1}/4</span>
          </div>
          <div className="wizardBody">
            {(preferences.onboardingStep || 0) <= 0 && (
              <div>
                <strong>{"欢迎使用 AI8888 Switch"}</strong>
                <p className="muted">{"我们会帮你完成：检测端点 → 选择工具与 Key → 写入配置 → 完成。"}</p>
              </div>
            )}
            {(preferences.onboardingStep || 0) === 1 && (
              <div>
                <strong>{"检测可用端点"}</strong>
                <p className="muted">{"点击下方按钮，自动选择延迟最低的 AI8888 域名。"}</p>
                <div className="actions">
                  <button onClick={() => void probeBestEndpoint()} disabled={probingEndpoint}>{probingEndpoint ? "检测中" : "开始检测"}</button>
                </div>
                {endpointProbe && <p className="muted">{"当前端点："}{endpointProbe.selectedDomain}</p>}
              </div>
            )}
            {(preferences.onboardingStep || 0) === 2 && (
              <div>
                <strong>{"选择工具与 API Key"}</strong>
                <p className="muted">{"在下方面板选择目标工具，并选中或创建 API Key。"}</p>
                <p className="muted">{"当前工具："}{selectedTool?.displayName || state.selectedTool}{" ，Key："}{selectedKey?.name || (manualKey ? "手动 Key" : "未选择")}</p>
              </div>
            )}
            {(preferences.onboardingStep || 0) >= 3 && (
              <div>
                <strong>{"写入配置并完成"}</strong>
                <p className="muted">{"确认 Base URL、Key 与模型后，点击“写入配置”。也可先跳过，后续再写入。"}</p>
              </div>
            )}
          </div>
          <div className="actions wizardActions">
            <button className="ghost" onClick={() => void completeOnboarding()}>{"跳过引导"}</button>
            {(preferences.onboardingStep || 0) > 0 && (
              <button className="ghost" onClick={() => void saveOnboardingStep(Math.max(0, (preferences.onboardingStep || 0) - 1))}>{"上一步"}</button>
            )}
            {(preferences.onboardingStep || 0) < 3 ? (
              <button onClick={() => void saveOnboardingStep(Math.min(3, (preferences.onboardingStep || 0) + 1))}>{"下一步"}</button>
            ) : (
              <button onClick={() => void completeOnboarding()}>{"完成"}</button>
            )}
          </div>
        </section>
      )}

      <CodexOfficialAccount canActivateAi8888={Boolean(effectiveKey)} onActivateAi8888={activateAi8888ForCodex} onConfigChanged={async () => { await refreshConfigSnapshots(); await refreshLocalRouteManifest(); }} />

      <section className="panel quickActions"><div><h2>跨工具会话管理</h2><p className="muted">浏览、搜索并恢复 Codex、Claude、Gemini、OpenCode、OpenClaw 与 Hermes 本地会话。</p></div><button onClick={openCodexSessions}>{"\u6253\u5f00\u4f1a\u8bdd\u7ba1\u7406"}</button></section>

      <section className="grid two">
        <div className="panel"><div className="panelHead"><h2>订阅</h2><div className="actions"><button className="secondary mini" onClick={() => void openDailyReset()}>日卡重置</button><span className="badge">可用 {state.subscriptions.filter(isActiveSubscription).length} / 总计 {state.subscriptions.length}</span></div></div><div className="list">{state.subscriptions.length === 0 && <p className="muted">暂无订阅</p>}{state.subscriptions.map((sub) => { const progress = subscriptionProgressInfo(sub, state.subscriptionProgress); const daily = usageWindow(sub, progress, "daily"); const weekly = usageWindow(sub, progress, "weekly"); const monthly = usageWindow(sub, progress, "monthly"); return <article className="row subscriptionRow" key={sub.id}><div><strong>{progress?.groupName || sub.groupName || sub.group?.name || `订阅 #${sub.id}`}</strong><small>{sub.status} - 到期 {safeDate(progress?.expiresAt ?? sub.expiresAt)}</small><small>{quotaLine(sub, progress)}</small><div className="usageGrid"><span>日：已用 {moneyOrDash(daily.used)} / 限额 {moneyOrDash(daily.limit)} / 剩余 {moneyOrDash(daily.remaining)} / {percentLabel(daily.used, daily.limit, daily.percentage)}</span><span>周：已用 {moneyOrDash(weekly.used)} / 限额 {moneyOrDash(weekly.limit)} / 剩余 {moneyOrDash(weekly.remaining)} / {percentLabel(weekly.used, weekly.limit, weekly.percentage)}</span><span>月：已用 {moneyOrDash(monthly.used)} / 限额 {moneyOrDash(monthly.limit)} / 剩余 {moneyOrDash(monthly.remaining)} / {percentLabel(monthly.used, monthly.limit, monthly.percentage)}</span></div></div><span>{isActiveSubscription(sub) ? "有效" : "无效"}</span></article>; })}</div></div>
        <div className="panel"><div className="panelHead"><h2>API Key</h2><span className="badge">{state.keys.total}</span></div><div className="inlineForm"><input value={newKeyName} onChange={(e) => setNewKeyName(e.target.value)} placeholder="Key 名称" /><select value={newKeyGroupId} onChange={(e) => setNewKeyGroupId(e.target.value)}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}{group.platform ? ` - ${group.platform}` : ""}</option>)}</select><button onClick={createKey} disabled={busy || !newKeyName}>创建</button></div><div className="list keys">{state.keys.items.length === 0 && <p className="muted">暂无 Key。请先创建或同步 API Key。</p>}{state.keys.items.map((item) => { const resolvedGroup = keyGroup(item, state.groups); const groupValue = editKeyGroupId[item.id] ?? (keyGroupId(item)?.toString() ?? ""); return <article className={"row selectable " + (selectedKey?.id === item.id ? "selected" : "")} key={item.id} onClick={() => chooseKey(item.id)}><div><strong>{item.name || `Key #${item.id}`}</strong><small>{item.status || "unknown"} - {resolvedGroup?.name || "未分组"} - {maskKey(item.key)}</small></div><div className="actions"><select value={groupValue} onChange={(e) => { e.stopPropagation(); setEditKeyGroupId((cur) => ({ ...cur, [item.id]: e.target.value })); void updateKeyGroup(item.id, e.target.value); }} onClick={(e) => e.stopPropagation()}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}</select><button className="link" onClick={(e) => { e.stopPropagation(); void deleteKey(item.id); }}>删除</button></div></article>; })}</div></div>
      </section>

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
            return <article className={"row profileRow " + (selectedProfileId === profile.id ? "selected" : "")} key={profile.id}><div><strong>{profile.name}</strong><small>{profile.tool} · {profile.baseUrl} · {profile.model || "默认模型"} · 审核 {profile.reviewModel || "跟随主模型"}</small><small>{keyText} · {routeText}</small></div><div className="actions"><button className="ghost mini" onClick={() => { void loadProfileIntoForm(profile); }} disabled={busy || profileBusyId !== null}>载入表单</button><button className="secondary mini" onClick={() => { void applyConfigProfile(profile); }} disabled={busy || profileBusyId !== null}>{profileBusyId === profile.id ? "处理中" : "一键应用"}</button><button className="ghost mini" onClick={() => { void deleteConfigProfile(profile); }} disabled={busy || profileBusyId !== null}>删除</button></div></article>;
          })}
        </div>
      </section>

      <WorkspaceCenter
        profiles={profiles.map((profile) => ({ id: profile.id, name: profile.name }))}
        selectedProfileId={selectedProfileId}
        onProfileApplied={async (profileId) => {
          const profile = profiles.find((item) => item.id === profileId);
          if (profile) await applyConfigProfile(profile);
        }}
      />

      <section className="panel switchPanel">
        <div className="panelHead"><h2>写入配置</h2><span className="badge">{selectedTool?.displayName ?? state.selectedTool}</span></div>
        <div className="switchGrid">
          <label>目标工具<select value={state.selectedTool} onChange={(e) => chooseTool(e.target.value)}>{tools.map((tool) => <option key={tool.tool} value={tool.tool}>{tool.displayName}</option>)}</select></label>
          <label>接口 Base URL<div className="inputAction"><input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /><button className="secondary" onClick={probeBestEndpoint} disabled={probingEndpoint || busy}>{probingEndpoint ? "检测中" : "检测最优端点"}</button></div></label>
          {endpointProbe && <div className="wide endpointProbeResults">{endpointProbe.results.map((result) => <span className={result.selected ? "selectedEndpoint" : ""} key={result.domain}>{result.selected ? "已选 " : ""}{endpointProbeText(result)}</span>)}</div>}
          <label className="wide">API Key<input value={manualKey} onChange={(e) => setManualKey(e.target.value)} placeholder={selectedKey?.key ? "使用选中的 Key" : "sk-..."} /></label>
          <label className="wide">模型选择（可选）<button className="secondary" onClick={testModels} disabled={testingModels || !effectiveKey || !baseUrl.trim()}>获取模型列表</button><select value={selectedModel} onChange={(e) => setSelectedModel(e.target.value)} disabled={testingModels}>{!selectedModel && models.length === 0 ? <option value="">可不选</option> : null}{selectedModel && !models.some((model) => model.id === selectedModel) ? <option value={selectedModel}>{selectedModel} - Profile</option> : null}{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>
          {state.selectedTool === "codex" && <label className="wide">自动审核模型（可选，留空跟随主模型）<select value={selectedReviewModel} onChange={(e) => setSelectedReviewModel(e.target.value)} disabled={testingModels}><option value="">跟随主模型</option>{selectedReviewModel && !models.some((model) => model.id === selectedReviewModel) ? <option value={selectedReviewModel}>{selectedReviewModel} - Profile</option> : null}{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>}
          <label className="wide checkboxLine"><input type="checkbox" checked={routeCodexEnabled} onChange={(e) => setRouteCodexEnabled(e.target.checked)} /> 启用 Codex 本地路由（127.0.0.1:15888/v1 / PROXY_MANAGED）</label>
          <label className="wide checkboxLine"><input type="checkbox" checked={routeClaudeEnabled} onChange={(e) => setRouteClaudeEnabled(e.target.checked)} /> 启用 Claude 本地路由（127.0.0.1:15888 / PROXY_MANAGED）</label>
          <label className="wide checkboxLine"><input type="checkbox" checked={routeOpenCodeEnabled} onChange={(e) => setRouteOpenCodeEnabled(e.target.checked)} /> 启用 OpenCode 本地路由（127.0.0.1:15888/v1 / PROXY_MANAGED）</label>
          {routeClaudeEnabled && <div className="routeBox wide"><div className="routeTitle">Claude 路由模型映射</div><div className="switchGrid"><label>Sonnet<input value={localRouteModelMap.sonnet} onChange={(e) => updateLocalRouteModel("sonnet", e.target.value)} /></label><label>Opus<input value={localRouteModelMap.opus} onChange={(e) => updateLocalRouteModel("opus", e.target.value)} /></label><label>Haiku<input value={localRouteModelMap.haiku} onChange={(e) => updateLocalRouteModel("haiku", e.target.value)} /></label></div><div className="actions"><button className="secondary" onClick={fillLocalRouteModels} disabled={!modelChoice}>用当前模型填充</button><label className="checkboxLine"><input type="checkbox" checked={localRoutePreserveClaudeAuth} onChange={(e) => setLocalRoutePreserveClaudeAuth(e.target.checked)} /> 保留 Claude 现有认证</label></div></div>}
          {localRoutingEnabled && <label className="wide checkboxLine"><input type="checkbox" checked={localRouteOnly} onChange={(e) => setLocalRouteOnly(e.target.checked)} /> 只接管路由，不写模型</label>}
        </div>
        {selectedTool && <p className="muted">将写入：{selectedTool.configPath}。{selectedTool.notes}</p>}
        <div className="actions"><button onClick={writeSwitch} disabled={!canWrite || busy}>写入配置</button><button className="secondary" onClick={showPreview} disabled={busy || !effectiveKey}>预览目标</button><button className="ghost" onClick={copyKey} disabled={!effectiveKey}>复制 Key</button><button className="ghost" onClick={cleanupLocalRoute} disabled={busy}>清理本地路由</button><button className="ghost" onClick={restoreLocalRouteBackups} disabled={busy}>恢复旧版备份</button></div>
      </section>

      <section className="panel configHistory">
        <div className="panelHead"><h2>配置历史</h2><span className="badge">{configSnapshots.length}</span></div>
        <div className="list">
          {configSnapshots.length === 0 && <p className="muted">首次成功写入配置后会生成可回滚快照。</p>}
          {configSnapshots.map((snapshot) => <article className="row" key={snapshot.id}><div><strong>{snapshot.label}</strong><small>{safeTimestamp(snapshot.createdAt)} · {snapshot.files.length} 个文件</small></div><button className="ghost mini" onClick={() => { void restoreConfigSnapshot(snapshot.id); }} disabled={busy || restoringSnapshotId !== null}>{restoringSnapshotId === snapshot.id ? "恢复中" : "回滚"}</button></article>)}
        </div>
      </section>

      {preview.length > 0 && <section className="panel"><div className="panelHead"><h2>写入目标</h2></div><div className="list">{preview.map(([path, label]) => <article className="row" key={path}><div><strong>{label}</strong><small>{path}</small></div></article>)}</div></section>}
      {localRouteManifest && localRouteManifest.entries.length > 0 && <section className="panel routeManifest"><div className="panelHead"><h2>本地路由状态</h2></div>{localRouteManifest.entries.map((entry) => <div className="routeEntry" key={entry.app}><strong>{appLabel(entry.app)} - {entry.localBaseUrl}</strong><small>模型：{entry.model || "默认"}</small></div>)}{localRouteStatuses.map((status) => <div className={"routeEntry " + (status.detected ? "okEntry" : "")} key={status.app}><strong>{appLabel(status.app)}：{status.detected ? "已接管" : "未接管"}</strong><small>{status.detail}</small></div>)}</section>}
            <footer className="appFooter">
        <div>v0.1.0 Copyright AI8888.SHOP 2026</div>
        <div className="footerActions">
          <button className="ghost mini" onClick={() => { void checkUpdate(); }} disabled={checkingUpdate || installingUpdate}>{checkingUpdate ? "检查中" : "检查更新"}</button>
          {updateInfo?.updateAvailable && updateInfo.downloadUrl && <button className="secondary mini" onClick={() => { void installUpdate(); }} disabled={checkingUpdate || installingUpdate}>{installingUpdate ? "正在下载安装" : "下载并安装"}</button>}
          {(updateInfo?.downloadUrl || updateInfo?.releaseUrl) && <a href={updateInfo.downloadUrl || updateInfo.releaseUrl || "#"} target="_blank" rel="noreferrer">{updateInfo.updateAvailable ? (updateInfo.downloadUrl ? (updateInfo.downloadAccelerated ? "加速下载链接" : "直接下载链接") : "查看新版本") : "GitHub Releases"}</a>}
          {updateInfo?.updateAvailable && updateInfo?.releaseUrl && updateInfo?.downloadUrl && <a href={updateInfo.releaseUrl} target="_blank" rel="noreferrer">发布页</a>}
        </div>
        {updateProgress && installingUpdate && <div className="updateProgress"><div><strong>{updateProgress.message}</strong><small>{updateProgress.totalBytes > 0 ? `${bytesLabel(updateProgress.downloadedBytes)} / ${bytesLabel(updateProgress.totalBytes)} · ${updateProgress.percent.toFixed(1)}%` : "正在连接"}</small></div><progress max="100" value={updateProgress.percent} />{["preparing", "downloading", "fallback"].includes(updateProgress.status) && <button className="ghost mini" onClick={() => { void cancelUpdate(); }}>取消</button>}</div>}
        {updateInfo && <div className="muted">{updateInfo.updateAvailable ? `发现新版本 ${updateInfo.latestVersion}${updateInfo.downloadAccelerated ? "（已启用 GitHub 加速下载）" : updateInfo.mainlandChina ? "（大陆网络，未找到安装包资源）" : ""}` : updateInfo.error ? `更新检查失败：${updateInfo.error}` : `当前已是最新版本 ${updateInfo.currentVersion}`}</div>}
        <div className="credits">{"\u81f4\u8c22\u5f00\u6e90\u9879\u76ee\uff1a"}<a href="https://github.com/jlcodes99/cockpit-tools" target="_blank" rel="noreferrer">cockpit-tools</a><a href="https://github.com/jlcodes99/cc-switch" target="_blank" rel="noreferrer">cc-switch</a><a href="https://github.com/Wei-Shaw/sub2api" target="_blank" rel="noreferrer">sub2api</a></div>
      </footer>
    </main>
  );
}

const RootApp = new URLSearchParams(window.location.search).get("view") === "sessions" ? CodexSessionsApp : App;

createRoot(document.getElementById("root")!).render(<React.StrictMode><RootApp /></React.StrictMode>);


