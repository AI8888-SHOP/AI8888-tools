export type GroupSummary = { id: number; name: string; platform?: string | null; quota?: number | null; dailyLimitUsd?: number | null; weeklyLimitUsd?: number | null; monthlyLimitUsd?: number | null };
export type UsageWindowProgress = { limitUsd?: number | null; usedUsd?: number | null; remainingUsd?: number | null; percentage?: number | null; windowStart?: string | null; resetsAt?: string | null; resetsInSeconds?: number | null };
export type SubscriptionProgress = { id: number; groupName?: string | null; expiresAt?: string | null; expiresInDays?: number | null; daily?: UsageWindowProgress | null; weekly?: UsageWindowProgress | null; monthly?: UsageWindowProgress | null };
export type SubscriptionSummary = { id: number; status: string; startsAt?: string | null; expiresAt?: string | null; quota?: number | null; remaining?: number | null; group?: GroupSummary | null; groupId?: number | null; groupName?: string | null; dailyUsageUsd?: number | null; weeklyUsageUsd?: number | null; monthlyUsageUsd?: number | null; dailyUsedUsd?: number | null; weeklyUsedUsd?: number | null; monthlyUsedUsd?: number | null; dailyLimitUsd?: number | null; weeklyLimitUsd?: number | null; monthlyLimitUsd?: number | null };
export type SubscriptionProgressInfo = { subscription: SubscriptionSummary; progress?: SubscriptionProgress | null };
export type UsageKind = "daily" | "weekly" | "monthly";
export type AccountAlertLevel = "info" | "warning" | "critical";
export type AccountAlert = { id: string; level: AccountAlertLevel; title: string; detail: string; action?: "purchase" | "refresh" };

export function money(value: number | null | undefined) { return "$" + Number(value ?? 0).toFixed(4); }
export function firstNumber(...values: Array<number | null | undefined>) { return values.find((value) => typeof value === "number" && Number.isFinite(value)); }
export function moneyOrDash(value: number | null | undefined) { return typeof value === "number" && Number.isFinite(value) ? money(value) : "-"; }
export function percentLabel(used?: number | null, limit?: number | null, percent?: number | null) {
  const raw = typeof percent === "number" && Number.isFinite(percent) ? percent : (typeof used === "number" && typeof limit === "number" && limit > 0 ? (used / limit) * 100 : undefined);
  return typeof raw === "number" && Number.isFinite(raw) ? `${Math.max(0, raw).toFixed(1)}%` : "-";
}
export function isActiveSubscription(sub: SubscriptionSummary) { return ["active", "valid", "enabled", "running"].includes((sub.status || "").toLowerCase()); }
export function subscriptionProgressInfo(sub: SubscriptionSummary, progressList: SubscriptionProgressInfo[]) { return progressList.find((item) => item.subscription?.id === sub.id)?.progress ?? null; }
export function usageWindow(sub: SubscriptionSummary, progress: SubscriptionProgress | null, kind: UsageKind) {
  const window = progress?.[kind] ?? null;
  const used = firstNumber(window?.usedUsd, kind === "daily" ? sub.dailyUsedUsd : kind === "weekly" ? sub.weeklyUsedUsd : sub.monthlyUsedUsd, kind === "daily" ? sub.dailyUsageUsd : kind === "weekly" ? sub.weeklyUsageUsd : sub.monthlyUsageUsd);
  const limit = firstNumber(window?.limitUsd, kind === "daily" ? sub.dailyLimitUsd : kind === "weekly" ? sub.weeklyLimitUsd : sub.monthlyLimitUsd, kind === "daily" ? sub.group?.dailyLimitUsd : kind === "weekly" ? sub.group?.weeklyLimitUsd : sub.group?.monthlyLimitUsd);
  const remaining = firstNumber(window?.remainingUsd, typeof used === "number" && typeof limit === "number" ? Math.max(0, limit - used) : undefined);
  return { used, limit, remaining, percentage: window?.percentage };
}
export function quotaLine(sub: SubscriptionSummary, progress: SubscriptionProgress | null) {
  const monthly = usageWindow(sub, progress, "monthly");
  const quota = firstNumber(sub.quota, sub.group?.quota, monthly.limit);
  const remaining = firstNumber(sub.remaining, monthly.remaining, typeof quota === "number" && typeof monthly.used === "number" ? Math.max(0, quota - monthly.used) : undefined);
  return `\u603b\u989d\u5ea6 ${moneyOrDash(quota)} / \u5269\u4f59 ${moneyOrDash(remaining)}`;
}

function daysUntil(value?: string | null) {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return Math.ceil((date.getTime() - Date.now()) / (24 * 60 * 60 * 1000));
}

function usagePercent(used?: number | null, limit?: number | null, percentage?: number | null) {
  if (typeof percentage === "number" && Number.isFinite(percentage)) return percentage;
  if (typeof used === "number" && typeof limit === "number" && limit > 0) return (used / limit) * 100;
  return null;
}

export function buildAccountAlerts(input: {
  balance?: number | null;
  subscriptions: SubscriptionSummary[];
  subscriptionProgress: SubscriptionProgressInfo[];
}) {
  const alerts: AccountAlert[] = [];
  const balance = Number(input.balance ?? 0);
  if (Number.isFinite(balance) && balance <= 1) {
    alerts.push({
      id: `balance-${balance.toFixed(4)}`,
      level: balance <= 0 ? "critical" : "warning",
      title: balance <= 0 ? "\u8d26\u6237\u4f59\u989d\u4e0d\u8db3" : "\u8d26\u6237\u4f59\u989d\u8f83\u4f4e",
      detail: `\u5f53\u524d\u4f59\u989d ${money(balance)}\uff0c\u5efa\u8bae\u5c3d\u5feb\u5145\u503c\u4ee5\u514d\u5f71\u54cd\u4f7f\u7528\u3002`,
      action: "purchase",
    });
  }

  for (const sub of input.subscriptions) {
    const progress = subscriptionProgressInfo(sub, input.subscriptionProgress);
    const name = progress?.groupName || sub.groupName || sub.group?.name || `\u8ba2\u9605 #${sub.id}`;
    const expiresAt = progress?.expiresAt ?? sub.expiresAt;
    const days = typeof progress?.expiresInDays === "number" ? progress.expiresInDays : daysUntil(expiresAt);
    if (typeof days === "number" && days <= 7) {
      alerts.push({
        id: `expire-${sub.id}-${expiresAt || days}`,
        level: days <= 2 ? "critical" : "warning",
        title: days < 0 ? `${name} \u5df2\u8fc7\u671f` : `${name} \u5373\u5c06\u5230\u671f`,
        detail: days < 0
          ? `\u5df2\u8fc7\u671f ${Math.abs(days)} \u5929\uff0c\u7eed\u8d39\u540e\u53ef\u6062\u590d\u989d\u5ea6\u3002`
          : `\u8fd8\u5269 ${days} \u5929\u5230\u671f\uff08${expiresAt || "-"}\uff09\u3002`,
        action: "purchase",
      });
    }

    for (const kind of ["daily", "weekly", "monthly"] as UsageKind[]) {
      const window = usageWindow(sub, progress, kind);
      const pct = usagePercent(window.used, window.limit, window.percentage);
      if (pct == null || pct < 80) continue;
      const label = kind === "daily" ? "\u65e5" : kind === "weekly" ? "\u5468" : "\u6708";
      alerts.push({
        id: `usage-${sub.id}-${kind}-${Math.round(pct)}`,
        level: pct >= 95 ? "critical" : "warning",
        title: `${name} ${label}\u989d\u5ea6\u4f7f\u7528\u8f83\u9ad8`,
        detail: `\u5df2\u7528 ${moneyOrDash(window.used)} / \u9650\u989d ${moneyOrDash(window.limit)}\uff08${percentLabel(window.used, window.limit, window.percentage)}\uff09`,
        action: "refresh",
      });
    }
  }

  const rank = { critical: 0, warning: 1, info: 2 } as const;
  return alerts.sort((a, b) => rank[a.level] - rank[b.level] || a.title.localeCompare(b.title, "zh-CN"));
}
