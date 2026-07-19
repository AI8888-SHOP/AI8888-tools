import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type CodexAuthStatus = {
  cliAvailable: boolean;
  cliVersion?: string | null;
  authenticated: boolean;
  authMethod: string;
  statusMessage: string;
  activeProvider: string;
  ai8888ConfigAvailable: boolean;
  configExists?: boolean;
  configValid?: boolean;
  configError?: string | null;
  configuredModel?: string | null;
  configuredReviewModel?: string | null;
  configuredBaseUrl?: string | null;
  configuredKeyId?: number | null;
  configuredKeyName?: string | null;
  credentialStore: string;
  configPath: string;
  loginRunning: boolean;
  loginMode?: string | null;
  loginMessage?: string | null;
  loginSucceeded?: boolean | null;
  loginOutput: string[];
};

export type CodexAuthController = {
  status: CodexAuthStatus | null;
  busy: boolean;
  error: string | null;
  message: string;
  refresh: (silent?: boolean) => Promise<CodexAuthStatus | null>;
  startLogin: (mode: "browser" | "device", autoActivate?: boolean) => Promise<CodexAuthStatus | null>;
  cancelLogin: () => Promise<CodexAuthStatus | null>;
  logout: () => Promise<CodexAuthStatus | null>;
  activateOfficial: () => Promise<boolean>;
  openDevicePage: () => Promise<void>;
  clearError: () => void;
};

type Options = {
  onConfigChanged?: () => Promise<void>;
};

export function useCodexAuth(options: Options = {}): CodexAuthController {
  const [status, setStatus] = useState<CodexAuthStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("正在检测 Codex CLI");
  const busyRef = useRef(false);
  const autoActivateRef = useRef(false);
  const autoActivationRunningRef = useRef(false);
  const requestGenerationRef = useRef(0);
  const onConfigChangedRef = useRef(options.onConfigChanged);

  useEffect(() => {
    onConfigChangedRef.current = options.onConfigChanged;
  }, [options.onConfigChanged]);

  const enterBusy = useCallback(() => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    return true;
  }, []);

  const leaveBusy = useCallback(() => {
    busyRef.current = false;
    setBusy(false);
  }, []);

  const refresh = useCallback(async (silent = false) => {
    const generation = ++requestGenerationRef.current;
    try {
      const next = await invoke<CodexAuthStatus>("app_get_codex_auth_status");
      if (generation !== requestGenerationRef.current) return null;
      setStatus(next);
      if (!silent || (!next.loginRunning && next.loginMessage)) {
        setMessage(next.loginMessage || next.statusMessage);
      }
      setError(null);
      return next;
    } catch (reason) {
      if (generation !== requestGenerationRef.current) return null;
      const text = reason instanceof Error ? reason.message : String(reason);
      setError(text);
      if (!silent) setMessage("Codex 登录状态检测失败");
      return null;
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Poll serially while the CLI owns an interactive login. A timeout is
  // scheduled only after the previous request finishes, so stale responses
  // cannot overwrite a newer status.
  useEffect(() => {
    if (!status?.loginRunning) return;
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      // Foreground actions such as cancel/logout own the latest status. Letting
      // a poll start during one of them can invalidate its generation token.
      if (!busyRef.current) await refresh(true);
      if (!disposed) timer = window.setTimeout(() => void poll(), 1200);
    };
    timer = window.setTimeout(() => void poll(), 1200);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [refresh, status?.loginRunning]);

  const activateOfficial = useCallback(async () => {
    if (!enterBusy()) return false;
    setError(null);
    try {
      await invoke("app_activate_codex_official");
      const next = await refresh(true);
      if (!next?.authenticated || next.activeProvider !== "official") {
        throw new Error("官方账户已登录，但 Codex 配置状态尚未确认，请重试");
      }
      try {
        await onConfigChangedRef.current?.();
      } catch (reason) {
        // The configuration transaction already succeeded; a stale snapshot
        // should not make the user repeat the activation.
        console.warn("failed to refresh configuration metadata", reason);
      }
      setMessage("Codex 已切换到 OpenAI 官方账户");
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
    } finally {
      leaveBusy();
    }
  }, [enterBusy, leaveBusy, refresh]);

  useEffect(() => {
    if (!autoActivateRef.current || !status || status.loginRunning) return;
    if (status.loginSucceeded === false) {
      autoActivateRef.current = false;
      return;
    }
    if (status.loginSucceeded === true && status.authenticated) {
      if (status.activeProvider === "official") {
        autoActivateRef.current = false;
        return;
      }
      if (autoActivationRunningRef.current) return;
      autoActivationRunningRef.current = true;
      void activateOfficial().then((succeeded) => {
        if (succeeded) autoActivateRef.current = false;
      }).finally(() => {
        autoActivationRunningRef.current = false;
      });
    }
  }, [activateOfficial, status]);

  const startLogin = useCallback(async (mode: "browser" | "device", autoActivate = true) => {
    if (!enterBusy()) return null;
    const generation = ++requestGenerationRef.current;
    setError(null);
    autoActivateRef.current = autoActivate;
    try {
      const next = await invoke<CodexAuthStatus>("app_start_codex_login", { mode });
      if (generation !== requestGenerationRef.current) return null;
      setStatus(next);
      setMessage(mode === "device" ? "设备码登录已启动" : "官方浏览器登录已启动");
      return next;
    } catch (reason) {
      if (generation !== requestGenerationRef.current) return null;
      autoActivateRef.current = false;
      setError(reason instanceof Error ? reason.message : String(reason));
      return null;
    } finally {
      leaveBusy();
    }
  }, [enterBusy, leaveBusy]);

  const cancelLogin = useCallback(async () => {
    if (!enterBusy()) return null;
    const generation = ++requestGenerationRef.current;
    autoActivateRef.current = false;
    try {
      const next = await invoke<CodexAuthStatus>("app_cancel_codex_login");
      if (generation !== requestGenerationRef.current) return null;
      setStatus(next);
      setMessage("正在取消登录");
      return next;
    } catch (reason) {
      if (generation !== requestGenerationRef.current) return null;
      setError(reason instanceof Error ? reason.message : String(reason));
      return null;
    } finally {
      leaveBusy();
    }
  }, [enterBusy, leaveBusy]);

  const logout = useCallback(async () => {
    if (!enterBusy()) return null;
    const generation = ++requestGenerationRef.current;
    autoActivateRef.current = false;
    setError(null);
    try {
      const next = await invoke<CodexAuthStatus>("app_logout_codex");
      if (generation !== requestGenerationRef.current) return null;
      setStatus(next);
      setMessage("Codex 官方账户已注销");
      return next;
    } catch (reason) {
      if (generation !== requestGenerationRef.current) return null;
      setError(reason instanceof Error ? reason.message : String(reason));
      return null;
    } finally {
      leaveBusy();
    }
  }, [enterBusy, leaveBusy]);

  const openDevicePage = useCallback(async () => {
    try {
      await invoke("app_open_codex_device_auth_page");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }, []);

  return {
    status,
    busy,
    error,
    message,
    refresh,
    startLogin,
    cancelLogin,
    logout,
    activateOfficial,
    openDevicePage,
    clearError: () => setError(null),
  };
}
