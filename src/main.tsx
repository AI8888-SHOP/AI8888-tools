import React, { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import "./styles.css";
import { isActiveSubscription, money, moneyOrDash, percentLabel, quotaLine, subscriptionProgressInfo, usageWindow, type GroupSummary, type SubscriptionProgress, type SubscriptionProgressInfo, type SubscriptionSummary } from "./subscription";

type AccountSummary = { id: number; email: string; username?: string | null; role?: string | null; balance: number; concurrency: number; status: string; runMode?: string | null };
type ApiKeySummary = { id: number; name: string; key?: string | null; status?: string | null; quota?: number | null; quotaUsed?: number | null; expiresAt?: string | null; groupId?: number | null; group?: GroupSummary | null };
type ModelSummary = { id: string; ownedBy?: string | null };
type Pagination<T> = { items: T[]; total: number };
type AppStateData = { session?: unknown | null; account?: AccountSummary | null; subscriptions: SubscriptionSummary[]; subscriptionProgress: SubscriptionProgressInfo[]; groups: GroupSummary[]; keys: Pagination<ApiKeySummary>; selectedTool: string; selectedKeyId?: number | null; loginWindowOpen: boolean; lastError?: string | null };
type ToolProfile = { tool: string; displayName: string; description: string; directory: string; configPath: string; notes?: string | null };
type SwitchTarget = { tool: string; profileName: string; baseUrl: string; apiKey: string; model?: string | null; tokenType?: string | null; localRoutingEnabled?: boolean; localRouteApps?: string[]; localRouteModelMap?: Record<string, string>; localRoutePreserveClaudeAuth?: boolean; localRouteOnly?: boolean };
type LocalRouteEntry = { app: string; enabled: boolean; upstreamName: string; localBaseUrl: string; localApiKey: string; model?: string | null; modelMap?: Record<string, string>; source: string };
type LocalRouteManifest = { profileName: string; updatedAt: number; entries: LocalRouteEntry[] };
type LocalRouteStatus = { app: string; detected: boolean; configPath: string; baseUrlMatched: boolean; proxyKeyMatched: boolean; oauthPreserved?: boolean; mcpPreserved?: boolean; detail: string };
type EndpointProbeResult = { domain: string; baseUrl: string; attempts: number; successCount: number; packetLoss: number; averageLatencyMs?: number | null; bestLatencyMs?: number | null; selected: boolean; error?: string | null };
type EndpointProbeSummary = { selectedBaseUrl: string; selectedDomain: string; results: EndpointProbeResult[] };
type UpdateCheckResult = { currentVersion: string; latestVersion?: string | null; updateAvailable: boolean; releaseUrl?: string | null; repository: string; error?: string | null };
type CodexSessionMeta = { sessionId: string; title?: string | null; summary?: string | null; projectDir?: string | null; createdAt?: string | null; lastActiveAt?: string | null; sourcePath: string; resumeCommand: string; archived: boolean; modifiedAt: number };
type CodexSessionMessage = { role: string; content: string; timestamp?: string | null };
type CodexSessionVisibilityRepairOutcome = { sessionId: string; sourcePath: string; success: boolean; changed: boolean; error?: string | null };

const defaultState: AppStateData = { account: null, subscriptions: [], subscriptionProgress: [], groups: [], keys: { items: [], total: 0 }, selectedTool: "codex", selectedKeyId: null, loginWindowOpen: false };

function safeDate(value?: string | null) { if (!value) return "-"; const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString(); }
function maskKey(value?: string | null) { if (!value) return "\u672a\u8fd4\u56de\u660e\u6587"; if (value.length <= 12) return value; return `${value.slice(0, 8)}...${value.slice(-4)}`; }
function keyGroupId(key?: ApiKeySummary | null) { return key?.group?.id ?? key?.groupId ?? null; }
function keyGroup(key: ApiKeySummary | null | undefined, groups: GroupSummary[]) { return key?.group ?? groups.find((group) => group.id === key?.groupId) ?? null; }

function appLabel(app: string) { return app === "codex" ? "Codex" : app === "claude" ? "Claude" : app === "opencode" ? "OpenCode" : app; }

function endpointProbeText(result: EndpointProbeResult) {
  const loss = `${Math.round(result.packetLoss * 100)}%`;
  const avg = result.averageLatencyMs == null ? "-" : `${Math.round(result.averageLatencyMs)}ms`;
  return `${result.domain}：丢包 ${loss}，平均延迟 ${avg}`;
}

function AuthGate(props: { email: string; password: string; setEmail: (v: string) => void; setPassword: (v: string) => void; busy: boolean; message: string; error: string | null; onLogin: () => void; onOpenPurchase: () => void; onOpenRadar: () => void; onOpenModelStatus: () => void }) {
  return (
    <main className="shell authShell">
      <section className="hero authHero">
        <div>
          <p className="eyebrow">AI8888 Switch</p>
          <h1>请先登录 AI8888 账户</h1>
          <p className="heroText">登录后进入配置界面，并同步显示订阅、分组、API Key 和账户余额。</p>
        </div>
        <div className="statusCard authCard">
          <div>
            <strong>未登录</strong>
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
      <footer className="appFooter">v0.0.1 Copyright AI8888.SHOP 2026</footer>
    </main>
  );
}


function sessionTitle(session: CodexSessionMeta) {
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
  if (normalized === "assistant") return "Codex";
  if (normalized === "tool") return "\u5de5\u5177";
  return role || "\u6d88\u606f";
}

function CodexSessionsApp() {
  const [sessions, setSessions] = useState<CodexSessionMeta[]>([]);
  const [messages, setMessages] = useState<CodexSessionMessage[]>([]);
  const [selected, setSelected] = useState<CodexSessionMeta | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("\u5c31\u7eea");

  function applySessions(next: CodexSessionMeta[]) {
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

  async function loadSessions(okMessage?: string) {
    setBusy(true); setError(null);
    try {
      const next = await invoke<CodexSessionMeta[]>("app_list_codex_sessions");
      applySessions(next);
      setMessage(okMessage ?? `\u5df2\u52a0\u8f7d ${next.length} \u4e2a Codex \u4f1a\u8bdd`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function loadMessages(session: CodexSessionMeta | null) {
    if (!session) { setMessages([]); return; }
    setError(null);
    try {
      setMessages(await invoke<CodexSessionMessage[]>("app_get_codex_session_messages", { sourcePath: session.sourcePath }));
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
    const text = query.trim().toLowerCase();
    if (!text) return sessions;
    return sessions.filter((session) => [session.sessionId, session.title, session.summary, session.projectDir, session.sourcePath].some((value) => (value || "").toLowerCase().includes(text)));
  }, [query, sessions]);

  const selectedBatch = useMemo(() => sessions.filter((session) => selectedPaths.has(session.sourcePath)), [sessions, selectedPaths]);
  const actionTargets = selectedBatch.length > 0 ? selectedBatch : (selected ? [selected] : []);
  const allFilteredSelected = filtered.length > 0 && filtered.every((session) => selectedPaths.has(session.sourcePath));

  function toggleSelection(session: CodexSessionMeta, checked: boolean) {
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

  async function launchSessions(targets: CodexSessionMeta[]) {
    if (targets.length === 0) return;
    setBusy(true); setError(null);
    const failed: string[] = [];
    try {
      for (const target of targets) {
        try {
          await invoke("app_launch_codex_session", { sessionId: target.sessionId, cwd: target.projectDir ?? null });
        } catch (err) {
          failed.push(`${target.resumeCommand}  # ${String(err)}`);
        }
      }
      if (failed.length > 0) {
        await navigator.clipboard.writeText(failed.join("\n"));
        setError(`\u6709 ${failed.length} \u4e2a\u4f1a\u8bdd\u542f\u52a8\u5931\u8d25\uff0c\u5df2\u590d\u5236\u5931\u8d25\u9879\u7684\u6062\u590d\u547d\u4ee4\u3002`);
      }
      const successCount = targets.length - failed.length;
      setMessage(`\u5df2\u6253\u5f00 ${successCount} \u4e2a Codex \u6062\u590d\u7a97\u53e3`);
    } finally {
      setBusy(false);
    }
  }

  async function repairVisibility(targets: CodexSessionMeta[]) {
    if (targets.length === 0) return;
    setBusy(true); setError(null);
    try {
      const results = await invoke<CodexSessionVisibilityRepairOutcome[]>("app_repair_codex_session_visibility", { requests: targets.map((session) => ({ sessionId: session.sessionId, sourcePath: session.sourcePath })) });
      const failed = results.filter((item) => !item.success);
      const changed = results.filter((item) => item.success && item.changed).length;
      const unchanged = results.filter((item) => item.success && !item.changed).length;
      if (failed.length > 0) {
        setError(`\u6709 ${failed.length} \u4e2a\u4f1a\u8bdd\u4fee\u590d\u5931\u8d25\uff1a${failed[0].error || "\u672a\u77e5\u9519\u8bef"}`);
      }
      const next = await invoke<CodexSessionMeta[]>("app_list_codex_sessions");
      applySessions(next);
      setMessage(`\u5df2\u4fee\u590d ${changed} \u4e2a\u4f1a\u8bdd\u53ef\u89c1\u6027\uff0c${unchanged} \u4e2a\u65e0\u9700\u4fee\u590d`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => { void loadSessions(); }, []);
  useEffect(() => { void loadMessages(selected); }, [selected?.sourcePath]);

  return (
    <main className="shell sessionsShell">
      <section className="hero sessionsHero">
        <div><p className="eyebrow">Codex</p><h1>{"Codex \u4f1a\u8bdd\u7ba1\u7406"}</h1><p className="heroText">{"\u6d4f\u89c8\u672c\u5730 Codex \u4f1a\u8bdd\u8bb0\u5f55\uff0c\u5e76\u6062\u590d\u9009\u4e2d\u7684\u5bf9\u8bdd\u3002"}</p></div>
        <div className="statusCard"><span className="dot ok" /><div><strong>{sessions.length}{" \u4e2a\u4f1a\u8bdd"}</strong><small>{message}</small></div></div>
      </section>
      {error && <div className="alert">{error}</div>}
      <section className="sessionsGrid">
        <aside className="panel sessionsListPanel">
          <div className="panelHead"><h2>{"\u4f1a\u8bdd\u5217\u8868"}</h2><button className="ghost mini" onClick={() => loadSessions()} disabled={busy}>{busy ? "\u5904\u7406\u4e2d" : "\u5237\u65b0"}</button></div>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="\u641c\u7d22\u4f1a\u8bdd" />
          <div className="batchBar">
            <span>{"\u5df2\u9009 "}{selectedBatch.length}</span>
            <button className="ghost mini" onClick={toggleFilteredSelection} disabled={filtered.length === 0}>{allFilteredSelected ? "\u53d6\u6d88\u5f53\u524d" : "\u5168\u9009\u5f53\u524d"}</button>
            <button className="secondary mini" onClick={() => launchSessions(actionTargets)} disabled={busy || actionTargets.length === 0}>{"\u6062\u590d\u9009\u4e2d"}</button>
            <button className="ghost mini" onClick={() => repairVisibility(actionTargets)} disabled={busy || actionTargets.length === 0}>{"\u4fee\u590d\u53ef\u89c1\u6027"}</button>
          </div>
          <div className="list sessionsList">
            {filtered.length === 0 && <p className="muted">{"\u672a\u627e\u5230 Codex \u4f1a\u8bdd\u3002"}</p>}
            {filtered.map((session) => {
              const checked = selectedPaths.has(session.sourcePath);
              return <article className={"sessionItem " + (selected?.sourcePath === session.sourcePath ? "selected " : "") + (checked ? "checked" : "")} key={session.sourcePath} onClick={() => setSelected(session)}>
                <input aria-label="\u9009\u62e9\u4f1a\u8bdd" type="checkbox" checked={checked} onChange={(event) => toggleSelection(session, event.target.checked)} onClick={(event) => event.stopPropagation()} />
                <div><strong>{sessionTitle(session)}</strong><small>{displayTime(session.lastActiveAt ?? session.createdAt)}{session.archived ? " - \u5df2\u5f52\u6863" : ""}</small><small>{session.projectDir || session.sessionId}</small></div>
              </article>;
            })}
          </div>
        </aside>
        <section className="panel sessionDetailPanel">
          {!selected ? <div className="emptyState"><h2>{"\u9009\u62e9\u4e00\u4e2a\u4f1a\u8bdd"}</h2><p className="muted">{"\u9009\u4e2d Codex \u4f1a\u8bdd\u540e\uff0c\u53ef\u67e5\u770b\u6d88\u606f\u548c\u6062\u590d\u547d\u4ee4\u3002"}</p></div> : <>
            <div className="panelHead sessionDetailHead"><div><h2>{sessionTitle(selected)}</h2><p className="muted">{selected.projectDir || "\u672a\u8bb0\u5f55\u9879\u76ee\u76ee\u5f55"}</p></div><div className="actions"><button onClick={() => launchSessions([selected])} disabled={busy}>{"\u6062\u590d\u4f1a\u8bdd"}</button><button className="secondary" onClick={() => copy(selected.resumeCommand, "\u5df2\u590d\u5236\u6062\u590d\u547d\u4ee4")}>{"\u590d\u5236\u547d\u4ee4"}</button></div></div>
            <div className="resumeBox"><code>{selected.resumeCommand}</code><button className="ghost mini" onClick={() => copy(selected.sourcePath, "\u5df2\u590d\u5236\u6e90\u6587\u4ef6\u8def\u5f84")}>{"\u590d\u5236\u8def\u5f84"}</button></div>
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
  const [testingModels, setTestingModels] = useState(false);
  const [probingEndpoint, setProbingEndpoint] = useState(false);
  const [endpointProbe, setEndpointProbe] = useState<EndpointProbeSummary | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateCheckResult | null>(null);
  const [routeCodexEnabled, setRouteCodexEnabled] = useState(false);
  const [routeClaudeEnabled, setRouteClaudeEnabled] = useState(false);
  const [routeOpenCodeEnabled, setRouteOpenCodeEnabled] = useState(false);
  const [localRouteModelMap, setLocalRouteModelMap] = useState<Record<string, string>>({ sonnet: "", opus: "", haiku: "" });
  const [localRoutePreserveClaudeAuth, setLocalRoutePreserveClaudeAuth] = useState(false);
  const [localRouteOnly, setLocalRouteOnly] = useState(false);
  const [localRouteManifest, setLocalRouteManifest] = useState<LocalRouteManifest | null>(null);
  const [localRouteStatuses, setLocalRouteStatuses] = useState<LocalRouteStatus[]>([]);

  const selectedTool = useMemo(() => tools.find((tool) => tool.tool === state.selectedTool) ?? tools[0], [state.selectedTool, tools]);
  const selectedKey = useMemo(() => state.keys.items.find((key) => key.id === state.selectedKeyId) ?? state.keys.items[0], [state.keys.items, state.selectedKeyId]);
  const isAuthenticated = Boolean(state.session);
  const effectiveKey = manualKey.trim() || selectedKey?.key || "";
  const modelChoice = selectedModel.trim();
  const canWrite = Boolean(state.session && effectiveKey && baseUrl.trim());
  const localRoutingEnabled = routeCodexEnabled || routeClaudeEnabled || routeOpenCodeEnabled;
  const localRouteApps = useMemo(() => {
    const apps: string[] = [];
    if (routeCodexEnabled) apps.push("codex");
    if (routeClaudeEnabled) apps.push("claude");
    if (routeOpenCodeEnabled) apps.push("opencode");
    return apps;
  }, [routeCodexEnabled, routeClaudeEnabled, routeOpenCodeEnabled]);

  async function run<T>(action: () => Promise<T>, okMessage: string) {
    setBusy(true); setError(null);
    try { const result = await action(); setMessage(okMessage); return result; }
    catch (err) { const text = err instanceof Error ? err.message : String(err); setError(text); setMessage("操作失败"); throw err; }
    finally { setBusy(false); }
  }

  async function refreshLocalState() { const next = await invoke<AppStateData>("app_get_state"); setState({ ...defaultState, ...next }); }
  async function refreshLocalRouteManifest() { try { const [next, statuses] = await Promise.all([invoke<LocalRouteManifest>("app_get_local_route_manifest").catch(() => null), invoke<LocalRouteStatus[]>("app_get_local_route_statuses")]); setLocalRouteManifest(next); setLocalRouteStatuses(statuses); } catch { setLocalRouteManifest(null); setLocalRouteStatuses([]); } }
  async function refreshRemote() { const next = await run(() => invoke<AppStateData>("app_load_remote_state"), "已刷新余额、订阅与 Key"); if (next) setState({ ...defaultState, ...next }); }
  async function chooseTool(tool: string) { const next = await invoke<AppStateData>("app_set_selected_tool", { tool }); setState({ ...defaultState, ...next }); setModels([]); setSelectedModel(""); setModelError(null); }
  async function chooseKey(keyId: number) { const next = await invoke<AppStateData>("app_set_selected_key", { keyId }); setState({ ...defaultState, ...next }); const selectedItem = next.keys.items.find((item) => item.id === keyId); setNewKeyGroupId(keyGroupId(selectedItem)?.toString() ?? ""); setManualKey(selectedItem?.key ?? ""); }
  async function updateKeyGroup(keyId: number, groupId: string) { await run(() => invoke("app_update_key_group", { keyId, groupId: groupId ? Number(groupId) : null }), "已更新 Key 分组"); await refreshLocalState(); }
  async function createKey() { const created = await run(() => invoke<ApiKeySummary>("app_create_key", { payload: { name: newKeyName, groupId: newKeyGroupId ? Number(newKeyGroupId) : null } }), "API Key 已创建"); if (created?.key) setManualKey(created.key); await refreshLocalState(); }
  async function deleteKey(keyId: number) { await run(() => invoke("app_delete_key", { keyId }), "API Key 已删除"); await refreshLocalState(); }
  async function login() { const next = await run(() => invoke<AppStateData>("app_login_with_password", { email, password }), "登录成功"); if (next) { setState({ ...defaultState, ...next }); setPassword(""); await refreshLocalRouteManifest(); } }
  async function logout() { const next = await run(() => invoke<AppStateData>("app_logout"), "已退出登录"); if (next) { setState({ ...defaultState, ...next }); setManualKey(""); setModels([]); setSelectedModel(""); setModelError(null); } }
  async function openPurchase() { await run(() => invoke("app_open_purchase_window"), "已打开充值续费页面"); }
  async function openRadar() { await run(() => invoke("app_open_radar_window"), "已打开智商雷达"); }
  async function openModelStatus() { await run(() => invoke("app_open_model_status_window"), "已打开模型监控"); }

  async function openCodexSessions() { await run(() => invoke("app_open_codex_sessions_window"), "\u5df2\u6253\u5f00 Codex \u4f1a\u8bdd\u7ba1\u7406"); }

  async function checkUpdate() {
    setCheckingUpdate(true); setError(null);
    try {
      const result = await invoke<UpdateCheckResult>("app_check_update");
      setUpdateInfo(result);
      if (result.error) {
        setMessage(`\u68c0\u67e5\u66f4\u65b0\u5931\u8d25\uff1a${result.error}`);
      } else if (result.updateAvailable) {
        setMessage(`\u53d1\u73b0\u65b0\u7248\u672c ${result.latestVersion}`);
      } else {
        setMessage(`\u5f53\u524d\u5df2\u662f\u6700\u65b0\u7248\u672c ${result.currentVersion}`);
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setCheckingUpdate(false);
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
    return invoke<SwitchTarget>("app_prepare_switch", { tool: state.selectedTool, baseUrl, apiKey: effectiveKey, model: modelChoice || null, localRoutingEnabled, localRouteApps, localRouteModelMap, localRoutePreserveClaudeAuth, localRouteOnly });
  }

  async function showPreview() { const target = await run(prepareTarget, "已生成写入预览"); if (!target) return; setPreview(await invoke<[string, string][]>("app_copy_target_preview", { target })); }
  async function writeSwitch() { const target = await run(prepareTarget, "已生成写入目标"); if (!target) return; const artifacts = await run(() => invoke<[string, string][]>("app_write_switch", { target }), localRoutingEnabled ? "已写入配置并启动本地路由" : "已写入配置"); if (artifacts) setPreview(artifacts); await refreshLocalRouteManifest(); }
  async function copyKey() { if (!effectiveKey) return; await navigator.clipboard.writeText(effectiveKey); setMessage("API Key 已复制"); }
  async function cleanupLocalRoute() { const artifacts = await run(() => invoke<[string, string][]>("app_cleanup_local_route_takeover"), "已清理本地接管"); if (artifacts) setPreview(artifacts); await refreshLocalRouteManifest(); }
  async function restoreLocalRouteBackups() { const artifacts = await run(() => invoke<[string, string][]>("app_restore_local_route_backups"), "已从备份恢复配置"); if (artifacts) setPreview(artifacts); await refreshLocalRouteManifest(); }
  function updateLocalRouteModel(role: string, value: string) { setLocalRouteModelMap((current) => ({ ...current, [role]: value })); }
  function fillLocalRouteModels() { if (!modelChoice) return; setLocalRouteModelMap({ sonnet: modelChoice, opus: modelChoice, haiku: modelChoice }); }

  useEffect(() => { void (async () => { try { const [nextState, nextTools, nextEndpoint, nextManifest, nextStatuses] = await Promise.all([invoke<AppStateData>("app_get_state"), invoke<ToolProfile[]>("app_get_tools"), invoke<EndpointProbeSummary>("app_get_endpoint").catch(() => null), invoke<LocalRouteManifest>("app_get_local_route_manifest").catch(() => null), invoke<LocalRouteStatus[]>("app_get_local_route_statuses").catch(() => [])]); setState({ ...defaultState, ...nextState }); setTools(nextTools); if (nextEndpoint) { setEndpointProbe(nextEndpoint); setBaseUrl(nextEndpoint.selectedBaseUrl); setMessage(`\u5df2\u9009\u62e9\u53ef\u7528\u7aef\u70b9 ${nextEndpoint.selectedDomain}`); } setLocalRouteManifest(nextManifest); setLocalRouteStatuses(nextStatuses); setNewKeyGroupId(nextState.keys.items[0]?.group?.id?.toString() ?? ""); } catch (err) { setError(String(err)); } })(); }, []);
  useEffect(() => { setSelectedModel((current) => current && models.some((item) => item.id === current) ? current : (models[0]?.id ?? current)); }, [models]);
  useEffect(() => { if (selectedKey) setEditKeyGroupId((current) => ({ ...current, [selectedKey.id]: keyGroupId(selectedKey)?.toString() ?? "" })); }, [selectedKey]);

  if (!isAuthenticated) return <AuthGate email={email} password={password} setEmail={setEmail} setPassword={setPassword} busy={busy} message={message} error={error} onLogin={login} onOpenPurchase={openPurchase} onOpenRadar={openRadar} onOpenModelStatus={openModelStatus} />;

  return (
    <main className="shell">
      <section className="hero">
        <div><p className="eyebrow">AI8888 Switch</p><h1>切换 AI8888 API 配置</h1><p className="heroText">同步账户、订阅、分组与 API Key，一键写入 Codex、Claude、OpenCode 等工具。</p></div>
        <div className="statusCard"><span className="dot ok" /><div><strong>已登录</strong><small>{message}</small><small className="balanceLine">账户余额：{money(state.account?.balance ?? 0)} <button className="ghost mini inlineRefresh" onClick={refreshRemote} disabled={busy}>刷新</button></small><div className="statusActions"><button className="ghost mini statusButton" onClick={openPurchase}>充值续费</button><button className="ghost mini statusButton" onClick={openRadar}>智商雷达</button><button className="ghost mini statusButton" onClick={openModelStatus}>模型监控</button><button className="ghost mini statusButton" onClick={logout} disabled={busy}>退出登录</button></div></div></div>
      </section>
      {error && <div className="alert">{error}</div>}{modelError && <div className="alert">{modelError}</div>}
      <section className="panel quickActions"><div><h2>{"Codex \u4f1a\u8bdd\u7ba1\u7406"}</h2><p className="muted">{"\u6253\u5f00\u72ec\u7acb\u7a97\u53e3\u6d4f\u89c8\u672c\u5730 Codex \u4f1a\u8bdd\uff0c\u5e76\u6062\u590d\u9009\u4e2d\u7684\u5bf9\u8bdd\u3002"}</p></div><button onClick={openCodexSessions}>{"\u6253\u5f00\u4f1a\u8bdd\u7ba1\u7406"}</button></section>

      <section className="grid two">
        <div className="panel"><div className="panelHead"><h2>订阅</h2><span className="badge">可用 {state.subscriptions.filter(isActiveSubscription).length} / 总计 {state.subscriptions.length}</span></div><div className="list">{state.subscriptions.length === 0 && <p className="muted">暂无订阅</p>}{state.subscriptions.map((sub) => { const progress = subscriptionProgressInfo(sub, state.subscriptionProgress); const daily = usageWindow(sub, progress, "daily"); const weekly = usageWindow(sub, progress, "weekly"); const monthly = usageWindow(sub, progress, "monthly"); return <article className="row subscriptionRow" key={sub.id}><div><strong>{progress?.groupName || sub.groupName || sub.group?.name || `订阅 #${sub.id}`}</strong><small>{sub.status} - 到期 {safeDate(progress?.expiresAt ?? sub.expiresAt)}</small><small>{quotaLine(sub, progress)}</small><div className="usageGrid"><span>日：已用 {moneyOrDash(daily.used)} / 限额 {moneyOrDash(daily.limit)} / 剩余 {moneyOrDash(daily.remaining)} / {percentLabel(daily.used, daily.limit, daily.percentage)}</span><span>周：已用 {moneyOrDash(weekly.used)} / 限额 {moneyOrDash(weekly.limit)} / 剩余 {moneyOrDash(weekly.remaining)} / {percentLabel(weekly.used, weekly.limit, weekly.percentage)}</span><span>月：已用 {moneyOrDash(monthly.used)} / 限额 {moneyOrDash(monthly.limit)} / 剩余 {moneyOrDash(monthly.remaining)} / {percentLabel(monthly.used, monthly.limit, monthly.percentage)}</span></div></div><span>{isActiveSubscription(sub) ? "有效" : "无效"}</span></article>; })}</div></div>
        <div className="panel"><div className="panelHead"><h2>API Key</h2><span className="badge">{state.keys.total}</span></div><div className="inlineForm"><input value={newKeyName} onChange={(e) => setNewKeyName(e.target.value)} placeholder="Key 名称" /><select value={newKeyGroupId} onChange={(e) => setNewKeyGroupId(e.target.value)}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}{group.platform ? ` - ${group.platform}` : ""}</option>)}</select><button onClick={createKey} disabled={busy || !newKeyName}>创建</button></div><div className="list keys">{state.keys.items.length === 0 && <p className="muted">暂无 Key。请先创建或同步 API Key。</p>}{state.keys.items.map((item) => { const resolvedGroup = keyGroup(item, state.groups); const groupValue = editKeyGroupId[item.id] ?? (keyGroupId(item)?.toString() ?? ""); return <article className={"row selectable " + (selectedKey?.id === item.id ? "selected" : "")} key={item.id} onClick={() => chooseKey(item.id)}><div><strong>{item.name || `Key #${item.id}`}</strong><small>{item.status || "unknown"} - {resolvedGroup?.name || "未分组"} - {maskKey(item.key)}</small></div><div className="actions"><select value={groupValue} onChange={(e) => { e.stopPropagation(); setEditKeyGroupId((cur) => ({ ...cur, [item.id]: e.target.value })); void updateKeyGroup(item.id, e.target.value); }} onClick={(e) => e.stopPropagation()}><option value="">不绑定分组</option>{state.groups.map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}</select><button className="link" onClick={(e) => { e.stopPropagation(); void deleteKey(item.id); }}>删除</button></div></article>; })}</div></div>
      </section>

      <section className="panel switchPanel">
        <div className="panelHead"><h2>写入配置</h2><span className="badge">{selectedTool?.displayName ?? state.selectedTool}</span></div>
        <div className="switchGrid">
          <label>目标工具<select value={state.selectedTool} onChange={(e) => chooseTool(e.target.value)}>{tools.map((tool) => <option key={tool.tool} value={tool.tool}>{tool.displayName}</option>)}</select></label>
          <label>接口 Base URL<div className="inputAction"><input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /><button className="secondary" onClick={probeBestEndpoint} disabled={probingEndpoint || busy}>{probingEndpoint ? "检测中" : "检测最优端点"}</button></div></label>
          {endpointProbe && <div className="wide endpointProbeResults">{endpointProbe.results.map((result) => <span className={result.selected ? "selectedEndpoint" : ""} key={result.domain}>{result.selected ? "已选 " : ""}{endpointProbeText(result)}</span>)}</div>}
          <label className="wide">API Key<input value={manualKey} onChange={(e) => setManualKey(e.target.value)} placeholder={selectedKey?.key ? "使用选中的 Key" : "sk-..."} /></label>
          <label className="wide">模型选择（可选）<button className="secondary" onClick={testModels} disabled={testingModels || !effectiveKey || !baseUrl.trim()}>获取模型列表</button><select value={selectedModel} onChange={(e) => setSelectedModel(e.target.value)} disabled={models.length === 0 || testingModels}>{models.length === 0 ? <option value="">可不选</option> : null}{models.map((model) => <option key={model.id} value={model.id}>{model.id}{model.ownedBy ? ` - ${model.ownedBy}` : ""}</option>)}</select></label>
          <label className="wide checkboxLine"><input type="checkbox" checked={routeCodexEnabled} onChange={(e) => setRouteCodexEnabled(e.target.checked)} /> 启用 Codex 本地路由（127.0.0.1:15888/v1 / PROXY_MANAGED）</label>
          <label className="wide checkboxLine"><input type="checkbox" checked={routeClaudeEnabled} onChange={(e) => setRouteClaudeEnabled(e.target.checked)} /> 启用 Claude 本地路由（127.0.0.1:15888 / PROXY_MANAGED）</label>
          <label className="wide checkboxLine"><input type="checkbox" checked={routeOpenCodeEnabled} onChange={(e) => setRouteOpenCodeEnabled(e.target.checked)} /> 启用 OpenCode 本地路由（127.0.0.1:15888/v1 / PROXY_MANAGED）</label>
          {routeClaudeEnabled && <div className="routeBox wide"><div className="routeTitle">Claude 路由模型映射</div><div className="switchGrid"><label>Sonnet<input value={localRouteModelMap.sonnet} onChange={(e) => updateLocalRouteModel("sonnet", e.target.value)} /></label><label>Opus<input value={localRouteModelMap.opus} onChange={(e) => updateLocalRouteModel("opus", e.target.value)} /></label><label>Haiku<input value={localRouteModelMap.haiku} onChange={(e) => updateLocalRouteModel("haiku", e.target.value)} /></label></div><div className="actions"><button className="secondary" onClick={fillLocalRouteModels} disabled={!modelChoice}>用当前模型填充</button><label className="checkboxLine"><input type="checkbox" checked={localRoutePreserveClaudeAuth} onChange={(e) => setLocalRoutePreserveClaudeAuth(e.target.checked)} /> 保留 Claude 现有认证</label></div></div>}
          {localRoutingEnabled && <label className="wide checkboxLine"><input type="checkbox" checked={localRouteOnly} onChange={(e) => setLocalRouteOnly(e.target.checked)} /> 只接管路由，不写模型</label>}
        </div>
        {selectedTool && <p className="muted">将写入：{selectedTool.configPath}。{selectedTool.notes}</p>}
        <div className="actions"><button onClick={writeSwitch} disabled={!canWrite || busy}>写入配置</button><button className="secondary" onClick={showPreview} disabled={busy || !effectiveKey}>预览目标</button><button className="ghost" onClick={copyKey} disabled={!effectiveKey}>复制 Key</button><button className="ghost" onClick={cleanupLocalRoute} disabled={busy}>清理本地路由</button><button className="ghost" onClick={restoreLocalRouteBackups} disabled={busy}>恢复备份</button></div>
      </section>

      {preview.length > 0 && <section className="panel"><div className="panelHead"><h2>写入目标</h2></div><div className="list">{preview.map(([path, label]) => <article className="row" key={path}><div><strong>{label}</strong><small>{path}</small></div></article>)}</div></section>}
      {localRouteManifest && localRouteManifest.entries.length > 0 && <section className="panel routeManifest"><div className="panelHead"><h2>本地路由状态</h2></div>{localRouteManifest.entries.map((entry) => <div className="routeEntry" key={entry.app}><strong>{appLabel(entry.app)} - {entry.localBaseUrl}</strong><small>模型：{entry.model || "默认"}</small></div>)}{localRouteStatuses.map((status) => <div className={"routeEntry " + (status.detected ? "okEntry" : "")} key={status.app}><strong>{appLabel(status.app)}：{status.detected ? "已接管" : "未接管"}</strong><small>{status.detail}</small></div>)}</section>}
            <footer className="appFooter">
        <div>v0.0.1 Copyright AI8888.SHOP 2026</div>
        <div className="footerActions"><button className="ghost mini" onClick={checkUpdate} disabled={checkingUpdate}>{checkingUpdate ? "\u68c0\u67e5\u4e2d" : "\u68c0\u67e5\u66f4\u65b0"}</button>{updateInfo?.releaseUrl && <a href={updateInfo.releaseUrl} target="_blank" rel="noreferrer">{updateInfo.updateAvailable ? "\u67e5\u770b\u65b0\u7248\u672c" : "GitHub Releases"}</a>}</div>
        {updateInfo && <div className="muted">{updateInfo.updateAvailable ? `\u53d1\u73b0\u65b0\u7248\u672c ${updateInfo.latestVersion}` : updateInfo.error ? `\u66f4\u65b0\u68c0\u67e5\u5931\u8d25\uff1a${updateInfo.error}` : `\u5f53\u524d\u5df2\u662f\u6700\u65b0\u7248\u672c ${updateInfo.currentVersion}`}</div>}
        <div className="credits">{"\u81f4\u8c22\u5f00\u6e90\u9879\u76ee\uff1a"}<a href="https://github.com/jlcodes99/cockpit-tools" target="_blank" rel="noreferrer">cockpit-tools</a><a href="https://github.com/jlcodes99/cc-switch" target="_blank" rel="noreferrer">cc-switch</a><a href="https://github.com/Wei-Shaw/sub2api" target="_blank" rel="noreferrer">sub2api</a></div>
      </footer>
    </main>
  );
}

const RootApp = new URLSearchParams(window.location.search).get("view") === "sessions" ? CodexSessionsApp : App;

createRoot(document.getElementById("root")!).render(<React.StrictMode><RootApp /></React.StrictMode>);


