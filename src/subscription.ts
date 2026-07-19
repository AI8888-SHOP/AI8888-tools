export type GroupSummary = { id: number; name: string; platform?: string | null; status?: string | null; subscriptionType?: string | null; quota?: number | null; dailyLimitUsd?: number | null; weeklyLimitUsd?: number | null; monthlyLimitUsd?: number | null };
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
export function isActiveSubscription(sub: SubscriptionSummary) {
  if (!["active", "valid", "enabled", "running"].includes((sub.status || "").toLowerCase())) return false;
  const now = Date.now();
  const startsAt = sub.startsAt ? new Date(sub.startsAt).getTime() : Number.NaN;
  const expiresAt = sub.expiresAt ? new Date(sub.expiresAt).getTime() : Number.NaN;
  if (typeof sub.remaining === "number" && sub.remaining <= 0) return false;
  return !(Number.isFinite(startsAt) && startsAt > now) && !(Number.isFinite(expiresAt) && expiresAt <= now);
}
export function groupSupportsTool(group: GroupSummary | null | undefined, tool: "codex" | "any") {
  if (tool === "any") return true;
  const platform = (group?.platform || "").trim().toLowerCase();
  if (!platform) return true;
  return ["openai", "codex", "chatgpt", "gpt", "universal", "all", "通用"].some((token) => platform.includes(token));
}
export function activeSubscriptionGroupIds(subscriptions: SubscriptionSummary[], tool: "codex" | "any" = "codex") {
  const seen = new Set<number>();
  return subscriptions
    .filter(isActiveSubscription)
    .sort((left, right) => {
      const leftTime = left.expiresAt ? new Date(left.expiresAt).getTime() : Number.POSITIVE_INFINITY;
      const rightTime = right.expiresAt ? new Date(right.expiresAt).getTime() : Number.POSITIVE_INFINITY;
      return leftTime - rightTime;
    })
    .filter((subscription) => groupSupportsTool(subscription.group, tool))
    .map((subscription) => subscription.group?.id ?? subscription.groupId ?? null)
    .filter((groupId): groupId is number => {
      if (typeof groupId !== "number" || groupId <= 0 || seen.has(groupId)) return false;
      seen.add(groupId);
      return true;
    });
}
export function recommendedSubscriptionGroupId(subscriptions: SubscriptionSummary[], tool: "codex" | "any" = "codex") {
  return activeSubscriptionGroupIds(subscriptions, tool)[0] ?? null;
}
export function prioritizeByActiveSubscription<T>(items: T[], subscriptions: SubscriptionSummary[], getGroupId: (item: T) => number | null, tool: "codex" | "any" = "codex") {
  const order = new Map(activeSubscriptionGroupIds(subscriptions, tool).map((groupId, index) => [groupId, index]));
  return items
    .map((item, index) => ({ item, index, subscriptionRank: order.get(getGroupId(item) ?? -1) ?? Number.POSITIVE_INFINITY }))
    .sort((left, right) => left.subscriptionRank - right.subscriptionRank || left.index - right.index)
    .map(({ item }) => item);
}
function normalizedGroupName(value?: string | null) { return (value || "").trim().toLowerCase(); }
export function subscriptionsWithResolvedGroups(subscriptions: SubscriptionSummary[], groups: GroupSummary[], progressList: SubscriptionProgressInfo[] = []) {
  const progressById = new Map(progressList.map((item) => [item.subscription?.id, item.progress]));
  return subscriptions.map((subscription) => {
    const progress = progressById.get(subscription.id);
    const progressExpired = typeof progress?.expiresInDays === "number" && progress.expiresInDays < 0;
    const reportedRemaining = [progress?.daily?.remainingUsd, progress?.weekly?.remainingUsd, progress?.monthly?.remainingUsd]
      .filter((value): value is number => typeof value === "number" && Number.isFinite(value));
    const progressRemaining = reportedRemaining.length > 0
      ? (reportedRemaining.every((value) => value <= 0) ? 0 : Math.max(...reportedRemaining))
      : undefined;
    const groupById = groups.find((group) => group.id === subscription.groupId);
    const candidateNames = [progress?.groupName, subscription.groupName, subscription.group?.name]
      .map(normalizedGroupName)
      .filter(Boolean);
    const groupByName = candidateNames
      .map((name) => groups.find((group) => normalizedGroupName(group.name) === name))
      .find((group): group is GroupSummary => Boolean(group));
    const embeddedGroup = subscription.group;
    const hasEmbeddedGroupId = typeof embeddedGroup?.id === "number" && embeddedGroup.id > 0;
    const group = hasEmbeddedGroupId ? embeddedGroup : (groupById ?? groupByName ?? embeddedGroup ?? null);
    return {
      ...subscription,
      expiresAt: progressExpired ? new Date(Date.now() - 1000).toISOString() : (progress?.expiresAt ?? subscription.expiresAt),
      remaining: progressRemaining ?? subscription.remaining,
      groupId: group?.id ?? subscription.groupId,
      group,
    };
  });
}
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
