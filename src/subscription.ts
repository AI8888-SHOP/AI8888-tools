export type GroupSummary = { id: number; name: string; platform?: string | null; quota?: number | null; dailyLimitUsd?: number | null; weeklyLimitUsd?: number | null; monthlyLimitUsd?: number | null };
export type UsageWindowProgress = { limitUsd?: number | null; usedUsd?: number | null; remainingUsd?: number | null; percentage?: number | null; windowStart?: string | null; resetsAt?: string | null; resetsInSeconds?: number | null };
export type SubscriptionProgress = { id: number; groupName?: string | null; expiresAt?: string | null; expiresInDays?: number | null; daily?: UsageWindowProgress | null; weekly?: UsageWindowProgress | null; monthly?: UsageWindowProgress | null };
export type SubscriptionSummary = { id: number; status: string; startsAt?: string | null; expiresAt?: string | null; quota?: number | null; remaining?: number | null; group?: GroupSummary | null; groupId?: number | null; groupName?: string | null; dailyUsageUsd?: number | null; weeklyUsageUsd?: number | null; monthlyUsageUsd?: number | null; dailyUsedUsd?: number | null; weeklyUsedUsd?: number | null; monthlyUsedUsd?: number | null; dailyLimitUsd?: number | null; weeklyLimitUsd?: number | null; monthlyLimitUsd?: number | null };
export type SubscriptionProgressInfo = { subscription: SubscriptionSummary; progress?: SubscriptionProgress | null };
export type UsageKind = "daily" | "weekly" | "monthly";

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
