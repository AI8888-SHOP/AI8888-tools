import { useState } from "react";
import type { CodexAuthController } from "./useCodexAuth";

type Props = {
  controller: CodexAuthController;
  canActivateAi8888?: boolean;
  onActivateAi8888?: () => Promise<void>;
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

export default function CodexOfficialAccount({ controller, canActivateAi8888 = false, onActivateAi8888 }: Props) {
  const [switchingAi8888, setSwitchingAi8888] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const { status } = controller;

  async function logout() {
    if (!window.confirm("确认注销当前 Codex 官方账户？CLI 和 IDE 扩展共享此登录状态。")) return;
    await controller.logout();
  }

  async function activateAi8888() {
    if (!onActivateAi8888 || switchingAi8888) return;
    setSwitchingAi8888(true);
    setLocalError(null);
    try {
      await onActivateAi8888();
      await controller.refresh(true);
    } catch (reason) {
      setLocalError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSwitchingAi8888(false);
    }
  }

  async function copyLoginOutput() {
    if (!status?.loginOutput.length) return;
    await navigator.clipboard.writeText(status.loginOutput.join("\n"));
  }

  const provider = status?.activeProvider || "custom";
  const authenticated = Boolean(status?.authenticated);
  const running = Boolean(status?.loginRunning);
  const busy = controller.busy || switchingAi8888;

  return (
    <section className="panel codexAccountPanel">
      <div className="panelHead codexAccountHead">
        <div>
          <h2>OpenAI / Codex 官方账户</h2>
          <p className="muted">认证由官方 Codex CLI 管理</p>
        </div>
        <span className={`badge ${provider === "official" ? "officialBadge" : ""}`}>{providerLabel(provider)}</span>
      </div>

      {(controller.error || localError) && <div className="inlineAlert">{controller.error || localError}</div>}

      <div className="codexAccountGrid">
        <div className="codexAccountState">
          <span className={`dot ${authenticated ? "ok" : ""}`} />
          <div>
            <strong>{status == null ? "正在检测 Codex CLI" : status.cliAvailable ? authMethodLabel(status.authMethod) : "未检测到 Codex CLI"}</strong>
            <small>{controller.message}</small>
          </div>
        </div>
        <dl className="codexAccountMeta">
          <div><dt>CLI</dt><dd>{status?.cliVersion || "不可用"}</dd></div>
          <div><dt>凭据存储</dt><dd>{credentialStoreLabel(status?.credentialStore || "default")}</dd></div>
          <div><dt>配置</dt><dd title={status?.configPath}>{status?.configPath || "-"}</dd></div>
        </dl>
      </div>

      <div className="actions codexAccountActions">
        {!authenticated && !running && <button onClick={() => void controller.startLogin("browser", true)} disabled={busy || !status?.cliAvailable}>使用 ChatGPT 登录</button>}
        {!authenticated && !running && <button className="secondary" onClick={() => void controller.startLogin("device", true)} disabled={busy || !status?.cliAvailable}>设备码登录</button>}
        {running && status?.loginMode === "device" && <button className="secondary" onClick={() => void controller.openDevicePage()} disabled={busy}>打开官方认证页面</button>}
        {running && <button className="ghost" onClick={() => void controller.cancelLogin()} disabled={busy}>取消登录</button>}
        {authenticated && provider !== "official" && <button onClick={() => void controller.activateOfficial()} disabled={busy}>切换到 OpenAI 官方</button>}
        {authenticated && provider === "official" && onActivateAi8888 && <button className="secondary" onClick={() => void activateAi8888()} disabled={busy || !canActivateAi8888}>切换到 AI8888</button>}
        {authenticated && <button className="ghost" onClick={() => void logout()} disabled={busy}>注销官方账户</button>}
        <button className="ghost" onClick={() => void controller.refresh()} disabled={busy || running}>刷新状态</button>
      </div>
      {authenticated && provider === "official" && onActivateAi8888 && !canActivateAi8888 && <p className="muted compactNote">选择一个可用的 AI8888 Key 后即可切换回来。</p>}

      {status?.loginOutput.length ? (
        <details className="codexLoginOutput">
          <summary>官方登录详情</summary>
          <div className="codexLoginOutputHead"><span>Codex CLI 输出</span><button className="ghost mini" onClick={() => void copyLoginOutput()}>复制</button></div>
          <pre>{status.loginOutput.join("\n")}</pre>
        </details>
      ) : null}
    </section>
  );
}
