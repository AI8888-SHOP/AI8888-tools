import { buildAccountAlerts, isActiveSubscription, money, quotaLine, usageWindow, type SubscriptionProgressInfo, type SubscriptionSummary } from "../src/subscription";

function assert(condition: unknown, message: string) {
  if (!condition) throw new Error(message);
}

const sub: SubscriptionSummary = {
  id: 1,
  status: "active",
  expiresAt: new Date(Date.now() + 2 * 24 * 60 * 60 * 1000).toISOString(),
  groupName: "Pro",
  dailyUsedUsd: 9,
  dailyLimitUsd: 10,
  monthlyUsedUsd: 40,
  monthlyLimitUsd: 100,
};

const progress: SubscriptionProgressInfo[] = [{
  subscription: sub,
  progress: {
    id: 1,
    groupName: "Pro",
    expiresAt: sub.expiresAt,
    expiresInDays: 2,
    daily: { usedUsd: 9, limitUsd: 10, percentage: 90 },
    monthly: { usedUsd: 40, limitUsd: 100, percentage: 40 },
  },
}];

assert(isActiveSubscription(sub), "active subscription");
assert(money(1.2) === "$1.2000", "money format");
const progressValue = progress[0].progress ?? null;
assert(quotaLine(sub, progressValue).includes("\u5269\u4f59"), "quota line");
const daily = usageWindow(sub, progressValue, "daily");
assert(daily.used === 9, "daily used");
const alerts = buildAccountAlerts({ balance: 0.5, subscriptions: [sub], subscriptionProgress: progress });
assert(alerts.some((item) => item.title.includes("\u4f59\u989d")), "balance alert");
assert(alerts.some((item) => item.title.includes("\u5230\u671f") || item.title.includes("\u989d\u5ea6")), "expiry/usage alert");
console.log("subscription logic tests passed");
