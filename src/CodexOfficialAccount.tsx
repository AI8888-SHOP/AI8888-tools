import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type CodexAuthStatus = {
  cliAvailable: boolean;
  cliVersion?: string | null;
  authenticated: boolean;
  authMethod: string;
  statusMessage: string;
  activeProvider: string;
  ai8888ConfigAvailable: boolean;
  credentialStore: string;
  configPath: string;
  loginRunning: boolean;
  loginMode?: string | null;
  loginMessage?: string | null;
  loginSucceeded?: boolean | null;
  loginOutput: string[];
};

type Props = {
  standalone?: boolean;
  canActivateAi8888?: boolean;
  onActivateAi8888?: () => Promise<void>;
  onConfigChanged?: () => Promise<void>;
};

function authMethodLabel(method: string) {
  if (method === "chatgpt") return "ChatGPT";
  if (method === "api_key") return "OpenAI API Key";
  if (method === "authenticated") return "已认证";
  if (method === "checking") return "登录中";
  return "未登录";
}

function providerLabel(provider: string) {
  if (provider === "official") return "OpenAI 官方";
  if (provider === "ai8888") return "AI8888";
  return "自定义 Provider";
}

function credentialStoreLabel(store: string) {
  if (store === "keyring") return "系统密钥库";
  if (store === "auto") return "自动安全存储";
  if (store === "file") return "auth.json";
  return "Codex 默认设置";
}

export default function CodexOfficialAccount({ standalone = false, canActivateAi8888 = false, onActivateAi8888, onConfigChanged }: Props) {
  const [status, setStatus] = useState<CodexAuthStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("正在检测 Codex CLI");
  const pendingAutoActivate = useRef(false);

  const refresh = useCallback(async (silent = false) => {
    try {
      const next = await invoke<CodexAuthStatus>("app_get_codex_auth_status");
      setStatus(next);
      if (!silent || (!next.loginRunning && next.loginMessage)) setMessage(next.loginMessage || next.statusMessage);
      setError(null);
      return next;
    } catch (reason) {
      const text = reason instanceof Error ? reason.message : String(reason);
      setError(text);
      if (!silent) setMessage("Codex 登录状态检测失败");
      return null;
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!status?.loginRunning) return;
    const timer = window.setInterval(() => {
      void refresh(true);
    }, 1200);
    return () => window.clearInterval(timer);
  }, [refresh, status?.loginRunning]);

  useEffect(() => {
    if (!pendingAutoActivate.current || status?.loginRunning) return;
    if (status?.loginSucceeded && status.authenticated) {
      pendingAutoActivate.current = false;
      void activateOfficial();
    } else if (status?.loginSucceeded === false) {
      pendingAutoActivate.current = false;
    }
  }, [status?.authenticated, status?.loginRunning, status?.loginSucceeded]);

  async function startLogin(mode: "browser" | "device") {
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<CodexAuthStatus>("app_start_codex_login", { mode });
      pendingAutoActivate.current = true;
      setStatus(next);
      setMessage(mode === "device" ? "设备码登录已启动" : "官方浏览器登录已启动");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function cancelLogin() {
    setBusy(true);
    pendingAutoActivate.current = false;
    try {
      setStatus(await invoke<CodexAuthStatus>("app_cancel_codex_login"));
      setMessage("正在取消登录");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function logout() {
    if (!window.confirm("确认注销当前 Codex 官方账户？CLI 和 IDE 扩展共享此登录状态。")) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await invoke<CodexAuthStatus>("app_logout_codex"));
      setMessage("Codex 官方账户已注销");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function activateOfficial() {
    setBusy(true);
    setError(null);
    try {
      await invoke("app_activate_codex_official");
      await refresh(true);
      await onConfigChanged?.();
      setMessage("Codex 已切换到 OpenAI 官方账户");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function activateAi8888() {
    if (!onActivateAi8888) return;
    setBusy(true);
    setError(null);
    try {
      await onActivateAi8888();
      await refresh(true);
      setMessage("Codex 已切换到 AI8888");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function openDevicePage() {
    try {
      await invoke("app_open_codex_device_auth_page");
    } catch (reason) {
      setError(String(reason));
    }
  }

  async function copyLoginOutput() {
    if (!status?.loginOutput.length) return;
    await navigator.clipboard.writeText(status.loginOutput.join("\n"));
    setMessage("登录信息已复制");
  }

  const provider = status?.activeProvider || "custom";
  const authenticated = Boolean(status?.authenticated);
  const running = Boolean(status?.loginRunning);

  return (
    <section className={`panel codexAccountPanel ${standalone ? "standaloneCodexAccount" : ""}`}>
      <div className="panelHead codexAccountHead">
        <div>
          <h2>OpenAI / Codex 官方账户</h2>
          <p className="muted">认证由官方 Codex CLI 管理</p>
        </div>
        <span className={`badge ${provider === "official" ? "officialBadge" : ""}`}>{providerLabel(provider)}</span>
      </div>

      {error && <div className="inlineAlert">{error}</div>}

      <div className="codexAccountGrid">
        <div className="codexAccountState">
          <span className={`dot ${authenticated ? "ok" : ""}`} />
          <div>
            <strong>{status?.cliAvailable ? authMethodLabel(status.authMethod) : "未检测到 Codex CLI"}</strong>
            <small>{message}</small>
          </div>
        </div>
        <dl className="codexAccountMeta">
          <div><dt>CLI</dt><dd>{status?.cliVersion || "不可用"}</dd></div>
          <div><dt>凭据存储</dt><dd>{credentialStoreLabel(status?.credentialStore || "default")}</dd></div>
          <div><dt>配置</dt><dd title={status?.configPath}>{status?.configPath || "-"}</dd></div>
        </dl>
      </div>

      <div className="actions codexAccountActions">
        {!authenticated && !running && <button onClick={() => void startLogin("browser")} disabled={busy || !status?.cliAvailable}>使用 ChatGPT 登录</button>}
        {!authenticated && !running && <button className="secondary" onClick={() => void startLogin("device")} disabled={busy || !status?.cliAvailable}>设备码登录</button>}
        {running && status?.loginMode === "device" && <button className="secondary" onClick={() => void openDevicePage()} disabled={busy}>打开官方认证页面</button>}
        {running && <button className="ghost" onClick={() => void cancelLogin()} disabled={busy}>取消登录</button>}
        {authenticated && provider !== "official" && <button onClick={() => void activateOfficial()} disabled={busy}>切换到 OpenAI 官方</button>}
        {authenticated && provider === "official" && onActivateAi8888 && <button className="secondary" onClick={() => void activateAi8888()} disabled={busy || !canActivateAi8888}>切换到 AI8888</button>}
        {authenticated && <button className="ghost" onClick={() => void logout()} disabled={busy}>注销官方账户</button>}
        <button className="ghost" onClick={() => void refresh()} disabled={busy || running}>刷新状态</button>
      </div>
      {authenticated && provider === "official" && onActivateAi8888 && !canActivateAi8888 && <p className="muted compactNote">选择一个可用的 AI8888 Key 后即可切换回来。</p>}

      {status?.loginOutput.length ? (
        <div className="codexLoginOutput">
          <div className="codexLoginOutputHead"><strong>官方登录信息</strong><button className="ghost mini" onClick={() => void copyLoginOutput()}>复制</button></div>
          <pre>{status.loginOutput.join("\n")}</pre>
        </div>
      ) : null}
    </section>
  );
}
