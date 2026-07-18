import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const APPS = ["codex", "claude", "gemini", "opencode", "openclaw", "hermes"] as const;
const APP_LABELS: Record<string, string> = { codex: "Codex", claude: "Claude", gemini: "Gemini", opencode: "OpenCode", openclaw: "OpenClaw", hermes: "Hermes" };

type ProfileOption = { id: string; name: string };
type McpServer = { id: string; name: string; transport: string; command: string; args: string[]; env: Record<string, string>; url: string; enabledApps: string[]; updatedAt: number };
type PromptPreset = { id: string; name: string; content: string; enabledApps: string[]; updatedAt: number };
type SkillPackage = { id: string; name: string; description: string; source: string; enabledApps: string[]; updatedAt: number };
type ProjectSnapshot = { id: string; name: string; profileId?: string | null; updatedAt: number };
type ProxyEndpoint = { id: string; name: string; baseUrl: string; priority: number; enabled: boolean };
type ProxySettings = { autoFailover: boolean; requestTimeoutMs: number; connectTimeoutMs: number; maxRetries: number; circuitFailureThreshold: number; circuitOpenSeconds: number; endpoints: ProxyEndpoint[] };
type ModelPrice = { model: string; inputPerMillion: number; outputPerMillion: number; cachedInputPerMillion: number };
type WorkspaceData = { mcpServers: McpServer[]; prompts: PromptPreset[]; skills: SkillPackage[]; projects: ProjectSnapshot[]; activeProjectId?: string | null; proxySettings: ProxySettings; modelPrices: ModelPrice[] };
type DiagnosticItem = { id: string; level: string; title: string; detail: string; path?: string | null };
type UsageDashboard = { totalRequests?: number; successfulRequests?: number; failedRequests?: number; inputTokens?: number; outputTokens?: number; cachedInputTokens?: number; totalCostUsd?: number; byModel?: Array<{ model: string; requests: number; inputTokens: number; outputTokens: number; costUsd: number }>; daily?: Array<{ date: string; requests: number; tokens: number; costUsd: number }> };
type EndpointHealth = { id?: string; name?: string; baseUrl?: string; healthy?: boolean; latencyMs?: number | null; status?: number | null; error?: string | null };

type WorkspaceCenterProps = {
  profiles: ProfileOption[];
  selectedProfileId: string | null;
  onProfileApplied: (profileId: string) => Promise<void>;
};

const emptyWorkspace: WorkspaceData = {
  mcpServers: [], prompts: [], skills: [], projects: [], activeProjectId: null, modelPrices: [],
  proxySettings: { autoFailover: true, requestTimeoutMs: 120000, connectTimeoutMs: 8000, maxRetries: 2, circuitFailureThreshold: 3, circuitOpenSeconds: 30, endpoints: [] },
};

function appToggles(enabled: string[], onChange: (apps: string[]) => void) {
  return <div className="appToggles">{APPS.map((app) => <label className="checkboxLine compact" key={app}><input type="checkbox" checked={enabled.includes(app)} onChange={(event) => onChange(event.target.checked ? [...new Set([...enabled, app])] : enabled.filter((item) => item !== app))} />{APP_LABELS[app]}</label>)}</div>;
}

function money(value?: number) { return `$${Number(value || 0).toFixed(4)}`; }
function count(value?: number) { return Number(value || 0).toLocaleString(); }

export default function WorkspaceCenter({ profiles, selectedProfileId, onProfileApplied }: WorkspaceCenterProps) {
  const [workspace, setWorkspace] = useState<WorkspaceData>(emptyWorkspace);
  const [tab, setTab] = useState("projects");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("工作区尚未载入");
  const [error, setError] = useState("");
  const [usage, setUsage] = useState<UsageDashboard | null>(null);
  const [diagnostics, setDiagnostics] = useState<DiagnosticItem[]>([]);
  const [health, setHealth] = useState<EndpointHealth[]>([]);

  const [projectName, setProjectName] = useState("");
  const [projectProfileId, setProjectProfileId] = useState(selectedProfileId || "");
  const [mcp, setMcp] = useState<McpServer>({ id: "", name: "", transport: "stdio", command: "", args: [], env: {}, url: "", enabledApps: ["codex"], updatedAt: 0 });
  const [mcpArgs, setMcpArgs] = useState("");
  const [mcpEnv, setMcpEnv] = useState("{}");
  const [prompt, setPrompt] = useState<PromptPreset>({ id: "", name: "", content: "", enabledApps: ["codex"], updatedAt: 0 });
  const [skillId, setSkillId] = useState("");
  const [skillName, setSkillName] = useState("");
  const [skillSource, setSkillSource] = useState("");
  const [skillDescription, setSkillDescription] = useState("");
  const [skillApps, setSkillApps] = useState<string[]>(["codex"]);
  const [importMcpApp, setImportMcpApp] = useState("codex");
  const [endpointLines, setEndpointLines] = useState("");
  const [price, setPrice] = useState<ModelPrice>({ model: "", inputPerMillion: 0, outputPerMillion: 0, cachedInputPerMillion: 0 });
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [importPath, setImportPath] = useState("");

  const load = useCallback(async () => {
    try {
      const data = await invoke<WorkspaceData>("app_get_workspace");
      setWorkspace(data);
      setEndpointLines(data.proxySettings.endpoints.map((item) => `${item.name}|${item.baseUrl}|${item.priority}|${item.enabled ? "on" : "off"}`).join("\n"));
      setMessage(`已载入 ${data.mcpServers.length} 个 MCP、${data.prompts.length} 个 Prompt、${data.skills.length} 个 Skill`);
      setError("");
    } catch (reason) { setError(String(reason)); }
  }, []);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => { if (selectedProfileId) setProjectProfileId(selectedProfileId); }, [selectedProfileId]);

  async function run<T>(operation: () => Promise<T>, success: string, apply?: (value: T) => void) {
    setBusy(true); setError("");
    try { const value = await operation(); apply?.(value); setMessage(success); return value; }
    catch (reason) { setError(String(reason)); return null; }
    finally { setBusy(false); }
  }

  async function saveMcp() {
    let env: Record<string, string> = {};
    try { env = JSON.parse(mcpEnv || "{}"); } catch { setError("MCP 环境变量必须是 JSON 对象"); return; }
    await run(() => invoke<WorkspaceData>("app_save_mcp_server", { server: { ...mcp, args: mcpArgs.split("\n").map((item) => item.trim()).filter(Boolean), env } }), "MCP 已保存并同步", setWorkspace);
  }

  function editMcp(item: McpServer) { setMcp(item); setMcpArgs(item.args.join("\n")); setMcpEnv(JSON.stringify(item.env || {}, null, 2)); }
  async function deleteMcp(id: string) { if (!window.confirm(`确认删除 MCP「${id}」并从所有应用移除？`)) return; await run(() => invoke<WorkspaceData>("app_delete_mcp_server", { id }), "MCP 已删除", setWorkspace); }
  async function importMcp() { await run(() => invoke<WorkspaceData>("app_import_mcp_from_app", { app: importMcpApp }), `已从 ${APP_LABELS[importMcpApp]} 导入 MCP`, setWorkspace); }

  async function savePrompt() { await run(() => invoke<WorkspaceData>("app_save_prompt", { prompt }), "Prompt 已保存并同步", setWorkspace); }
  async function deletePrompt(id: string) { if (!window.confirm("确认删除这个 Prompt？用户原有文件内容会保留。")) return; await run(() => invoke<WorkspaceData>("app_delete_prompt", { id }), "Prompt 已删除", setWorkspace); }

  async function installSkill() {
    await run(() => invoke<WorkspaceData>("app_install_skill", { id: skillId, name: skillName, source: skillSource, description: skillDescription, enabledApps: skillApps }), "Skill 已安装并同步", setWorkspace);
  }
  async function updateSkillApps(item: SkillPackage, apps: string[]) { await run(() => invoke<WorkspaceData>("app_update_skill_apps", { id: item.id, enabledApps: apps }), "Skill 同步范围已更新", setWorkspace); }
  async function deleteSkill(id: string) { if (!window.confirm("确认卸载这个 Skill？卸载前会自动备份。")) return; await run(() => invoke<WorkspaceData>("app_delete_skill", { id }), "Skill 已卸载并备份", setWorkspace); }

  async function saveProject(id?: string, name?: string, profileId?: string | null) {
    await run(() => invoke<WorkspaceData>("app_save_project", { id: id || null, name: name || projectName, profileId: profileId || projectProfileId || null }), "项目快照已保存", setWorkspace);
  }
  async function applyProject(item: ProjectSnapshot) {
    const project = await run(() => invoke<ProjectSnapshot>("app_apply_project", { id: item.id }), `项目「${item.name}」已切换`);
    if (project?.profileId) await onProfileApplied(project.profileId);
    await load();
  }
  async function deleteProject(id: string) { if (!window.confirm("确认删除这个项目快照？")) return; await run(() => invoke<WorkspaceData>("app_delete_project", { id }), "项目已删除", setWorkspace); }

  function parseEndpoints(): ProxyEndpoint[] {
    return endpointLines.split("\n").map((line, index) => {
      const [name = "endpoint", baseUrl = "", priority = String(index), enabled = "on"] = line.split("|").map((item) => item.trim());
      return { id: name.toLowerCase().replace(/[^a-z0-9._-]+/g, "-"), name, baseUrl, priority: Number(priority) || 0, enabled: enabled.toLowerCase() !== "off" };
    }).filter((item) => item.baseUrl);
  }
  async function saveProxy() { await run(() => invoke<WorkspaceData>("app_save_proxy_settings", { settings: { ...workspace.proxySettings, endpoints: parseEndpoints() } }), "代理与故障转移设置已保存", setWorkspace); }
  async function probeProxy() { await run(() => invoke<EndpointHealth[]>("app_probe_proxy_endpoints"), "端点健康检查完成", setHealth); }
  async function refreshUsage() { await run(() => invoke<UsageDashboard>("app_get_usage_dashboard", { days: 30 }), "用量统计已刷新", setUsage); }
  async function clearUsage() { if (!window.confirm("确认清空本地请求用量日志？")) return; await run(() => invoke("app_clear_usage"), "本地用量日志已清空", () => setUsage(null)); }
  async function savePrice() { await run(() => invoke<WorkspaceData>("app_save_model_price", { price }), "模型价格已保存", setWorkspace); }

  async function exportConfig() {
    const result = await run(() => invoke<{ path: string }>("app_export_config", { includeSecrets, passphrase }), "配置导出完成");
    if (result) setMessage(`配置已导出：${result.path}`);
  }
  async function importConfig() { await run(() => invoke<WorkspaceData>("app_import_config", { path: importPath, passphrase }), "配置已导入并同步", setWorkspace); }
  async function runDiagnostics() { await run(() => invoke<DiagnosticItem[]>("app_run_diagnostics"), "诊断完成", setDiagnostics); }
  async function repairWorkspace() { await run(() => invoke<WorkspaceData>("app_repair_workspace"), "工作区已修复并重新同步", setWorkspace); }

  const activeProject = useMemo(() => workspace.projects.find((item) => item.id === workspace.activeProjectId), [workspace]);
  const tabs = [
    ["projects", "项目"], ["mcp", "MCP"], ["prompts", "Prompts"], ["skills", "Skills"], ["proxy", "路由"], ["usage", "用量"], ["backup", "备份诊断"],
  ];

  return <section className="panel workspaceCenter">
    <div className="panelHead"><div><h2>工作区中心</h2><p className="muted workspaceSummary">{activeProject ? `当前项目：${activeProject.name}` : message}</p></div><span className="badge">统一管理</span></div>
    {error && <div className="inlineAlert">{error}</div>}
    <div className="segmented workspaceTabs" role="tablist">{tabs.map(([id, label]) => <button className={tab === id ? "active" : ""} onClick={() => setTab(id)} key={id} type="button">{label}</button>)}</div>

    {tab === "projects" && <div className="workspacePane">
      <div className="workspaceForm compactGrid"><label>项目名称<input value={projectName} onChange={(event) => setProjectName(event.target.value)} placeholder="开发 / 测试 / 写作" /></label><label>关联配置方案<select value={projectProfileId} onChange={(event) => setProjectProfileId(event.target.value)}><option value="">不切换 Profile</option>{profiles.map((item) => <option value={item.id} key={item.id}>{item.name}</option>)}</select></label><button onClick={() => void saveProject()} disabled={busy || !projectName.trim()}>保存当前工作区</button></div>
      <div className="list workspaceList">{workspace.projects.length === 0 && <p className="muted">暂无项目。项目会保存 Profile、MCP、Prompts 和 Skills 的当前组合。</p>}{workspace.projects.map((item) => <article className={`row ${item.id === workspace.activeProjectId ? "activeWorkspaceRow" : ""}`} key={item.id}><div><strong>{item.name}</strong><small>{profiles.find((profile) => profile.id === item.profileId)?.name || "不切换 Profile"}</small></div><div className="actions"><button className="secondary mini" onClick={() => void applyProject(item)} disabled={busy}>切换</button><button className="ghost mini" onClick={() => void saveProject(item.id, item.name, item.profileId)} disabled={busy}>更新快照</button><button className="ghost mini" onClick={() => void deleteProject(item.id)} disabled={busy}>删除</button></div></article>)}</div>
    </div>}

    {tab === "mcp" && <div className="workspacePane">
      <div className="workspaceToolbar"><select value={importMcpApp} onChange={(event) => setImportMcpApp(event.target.value)}>{APPS.map((app) => <option value={app} key={app}>{APP_LABELS[app]}</option>)}</select><button className="secondary" onClick={() => void importMcp()} disabled={busy}>从应用导入</button><button className="ghost" onClick={() => void invoke("app_sync_mcp_servers").then(load)} disabled={busy}>重新同步</button></div>
      <div className="workspaceForm formGrid"><label>ID<input value={mcp.id} onChange={(event) => setMcp({ ...mcp, id: event.target.value })} /></label><label>名称<input value={mcp.name} onChange={(event) => setMcp({ ...mcp, name: event.target.value })} /></label><label>传输<select value={mcp.transport} onChange={(event) => setMcp({ ...mcp, transport: event.target.value })}><option value="stdio">stdio</option><option value="http">HTTP</option><option value="sse">SSE</option></select></label>{mcp.transport === "stdio" ? <><label className="wide">命令<input value={mcp.command} onChange={(event) => setMcp({ ...mcp, command: event.target.value })} /></label><label>参数，每行一个<textarea value={mcpArgs} onChange={(event) => setMcpArgs(event.target.value)} /></label><label>环境变量 JSON<textarea value={mcpEnv} onChange={(event) => setMcpEnv(event.target.value)} /></label></> : <label className="wide">URL<input value={mcp.url} onChange={(event) => setMcp({ ...mcp, url: event.target.value })} /></label>}<div className="wide">{appToggles(mcp.enabledApps, (apps) => setMcp({ ...mcp, enabledApps: apps }))}</div><button onClick={() => void saveMcp()} disabled={busy || !mcp.id || !mcp.name}>保存并同步</button></div>
      <div className="list workspaceList">{workspace.mcpServers.map((item) => <article className="row" key={item.id}><div><strong>{item.name}</strong><small>{item.transport === "stdio" ? `${item.command} ${item.args.join(" ")}` : item.url}</small><small>{item.enabledApps.map((app) => APP_LABELS[app]).join(" / ") || "未启用"}</small></div><div className="actions"><button className="ghost mini" onClick={() => editMcp(item)}>编辑</button><button className="ghost mini" onClick={() => void deleteMcp(item.id)}>删除</button></div></article>)}</div>
    </div>}

    {tab === "prompts" && <div className="workspacePane"><div className="workspaceForm formGrid"><label>ID<input value={prompt.id} onChange={(event) => setPrompt({ ...prompt, id: event.target.value })} /></label><label>名称<input value={prompt.name} onChange={(event) => setPrompt({ ...prompt, name: event.target.value })} /></label><label className="wide">Markdown 内容<textarea className="promptEditor" value={prompt.content} onChange={(event) => setPrompt({ ...prompt, content: event.target.value })} /></label><div className="wide">{appToggles(prompt.enabledApps, (apps) => setPrompt({ ...prompt, enabledApps: apps }))}</div><button onClick={() => void savePrompt()} disabled={busy || !prompt.id || !prompt.name || !prompt.content}>保存并同步</button></div><div className="list workspaceList">{workspace.prompts.map((item) => <article className="row" key={item.id}><div><strong>{item.name}</strong><small>{item.content.slice(0, 120)}</small><small>{item.enabledApps.map((app) => APP_LABELS[app]).join(" / ") || "未启用"}</small></div><div className="actions"><button className="ghost mini" onClick={() => setPrompt(item)}>编辑</button><button className="ghost mini" onClick={() => void deletePrompt(item.id)}>删除</button></div></article>)}</div></div>}

    {tab === "skills" && <div className="workspacePane"><div className="workspaceForm formGrid"><label>ID<input value={skillId} onChange={(event) => setSkillId(event.target.value)} /></label><label>名称<input value={skillName} onChange={(event) => setSkillName(event.target.value)} /></label><label className="wide">来源<input value={skillSource} onChange={(event) => setSkillSource(event.target.value)} placeholder="本地目录、ZIP 或 GitHub 仓库 URL" /></label><label className="wide">说明<input value={skillDescription} onChange={(event) => setSkillDescription(event.target.value)} /></label><div className="wide">{appToggles(skillApps, setSkillApps)}</div><button onClick={() => void installSkill()} disabled={busy || !skillId || !skillName || !skillSource}>安装并同步</button></div><div className="list workspaceList">{workspace.skills.map((item) => <article className="row" key={item.id}><div><strong>{item.name}</strong><small>{item.description || item.source}</small>{appToggles(item.enabledApps, (apps) => void updateSkillApps(item, apps))}</div><button className="ghost mini" onClick={() => void deleteSkill(item.id)}>卸载</button></article>)}</div></div>}

    {tab === "proxy" && <div className="workspacePane"><div className="workspaceForm formGrid"><label className="checkboxLine"><input type="checkbox" checked={workspace.proxySettings.autoFailover} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, autoFailover: event.target.checked } })} />自动故障转移</label><label>请求超时（毫秒）<input type="number" value={workspace.proxySettings.requestTimeoutMs} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, requestTimeoutMs: Number(event.target.value) } })} /></label><label>连接超时（毫秒）<input type="number" value={workspace.proxySettings.connectTimeoutMs} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, connectTimeoutMs: Number(event.target.value) } })} /></label><label>最大重试<input type="number" value={workspace.proxySettings.maxRetries} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, maxRetries: Number(event.target.value) } })} /></label><label>熔断失败阈值<input type="number" value={workspace.proxySettings.circuitFailureThreshold} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, circuitFailureThreshold: Number(event.target.value) } })} /></label><label>熔断时长（秒）<input type="number" value={workspace.proxySettings.circuitOpenSeconds} onChange={(event) => setWorkspace({ ...workspace, proxySettings: { ...workspace.proxySettings, circuitOpenSeconds: Number(event.target.value) } })} /></label><label className="wide">备用端点，每行：名称 | Base URL | 优先级 | on/off<textarea value={endpointLines} onChange={(event) => setEndpointLines(event.target.value)} placeholder="backup|https://sub.ai8888.shop/v1|10|on" /></label><div className="actions wide"><button onClick={() => void saveProxy()} disabled={busy}>保存路由设置</button><button className="secondary" onClick={() => void probeProxy()} disabled={busy}>健康检查</button></div></div>{health.length > 0 && <div className="list workspaceList">{health.map((item, index) => <article className="row" key={item.id || item.baseUrl || index}><div><strong>{item.name || item.baseUrl}</strong><small>{item.healthy ? `正常 · ${item.latencyMs ?? "-"} ms` : item.error || `HTTP ${item.status || "-"}`}</small></div><span className={`healthDot ${item.healthy ? "healthy" : "failed"}`} /></article>)}</div>}</div>}

    {tab === "usage" && <div className="workspacePane"><div className="actions workspaceToolbar"><button onClick={() => void refreshUsage()} disabled={busy}>刷新 30 天统计</button><button className="ghost" onClick={() => void clearUsage()} disabled={busy}>清空本地日志</button></div>{usage && <><div className="metrics usageMetrics"><div><small>请求</small><strong>{count(usage.totalRequests)}</strong></div><div><small>成功率</small><strong>{usage.totalRequests ? `${((Number(usage.successfulRequests || 0) / usage.totalRequests) * 100).toFixed(1)}%` : "-"}</strong></div><div><small>Token</small><strong>{count(Number(usage.inputTokens || 0) + Number(usage.outputTokens || 0))}</strong></div><div><small>估算成本</small><strong>{money(usage.totalCostUsd)}</strong></div></div><div className="list workspaceList">{usage.byModel?.map((item) => <article className="row" key={item.model}><div><strong>{item.model}</strong><small>{count(item.requests)} 次 · 输入 {count(item.inputTokens)} · 输出 {count(item.outputTokens)}</small></div><span>{money(item.costUsd)}</span></article>)}</div></>}<div className="workspaceForm priceForm"><label>模型<input value={price.model} onChange={(event) => setPrice({ ...price, model: event.target.value })} /></label><label>输入 / 百万 Token<input type="number" value={price.inputPerMillion} onChange={(event) => setPrice({ ...price, inputPerMillion: Number(event.target.value) })} /></label><label>输出 / 百万 Token<input type="number" value={price.outputPerMillion} onChange={(event) => setPrice({ ...price, outputPerMillion: Number(event.target.value) })} /></label><label>缓存输入 / 百万 Token<input type="number" value={price.cachedInputPerMillion} onChange={(event) => setPrice({ ...price, cachedInputPerMillion: Number(event.target.value) })} /></label><button onClick={() => void savePrice()} disabled={busy || !price.model}>保存价格</button></div></div>}

    {tab === "backup" && <div className="workspacePane"><div className="workspaceForm formGrid"><label className="checkboxLine"><input type="checkbox" checked={includeSecrets} onChange={(event) => setIncludeSecrets(event.target.checked)} />包含密钥（强制加密）</label><label>导出/导入密码<input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} placeholder={includeSecrets ? "至少 8 个字符" : "加密导入时填写"} /></label><button onClick={() => void exportConfig()} disabled={busy || (includeSecrets && passphrase.length < 8)}>导出配置</button><label className="wide">导入文件路径<input value={importPath} onChange={(event) => setImportPath(event.target.value)} /></label><button className="secondary" onClick={() => void importConfig()} disabled={busy || !importPath}>导入并同步</button><div className="actions wide"><button className="ghost" onClick={() => void runDiagnostics()} disabled={busy}>运行诊断</button><button className="ghost" onClick={() => void repairWorkspace()} disabled={busy}>修复并重新同步</button></div></div><div className="list workspaceList">{diagnostics.map((item) => <article className={`row diagnostic-${item.level}`} key={item.id}><div><strong>{item.title}</strong><small>{item.detail}</small><small>{item.path}</small></div><span>{item.level.toUpperCase()}</span></article>)}</div></div>}
  </section>;
}
