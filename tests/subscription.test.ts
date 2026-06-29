import { isActiveSubscription, percentLabel, quotaLine, subscriptionProgressInfo, usageWindow, type SubscriptionProgressInfo, type SubscriptionSummary } from "../src/subscription";

declare const console: { log: (...args: unknown[]) => void };
function assertEqual<T>(actual: T, expected: T, message?: string) {
  if (actual !== expected) {
    throw new Error(`${message ?? "assertEqual failed"}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
function assertDeepEqual(actual: unknown, expected: unknown, message?: string) {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) {
    throw new Error(`${message ?? "assertDeepEqual failed"}: expected ${right}, got ${left}`);
  }
}

const sub: SubscriptionSummary = {
  id: 7,
  status: "active",
  dailyUsageUsd: 1.25,
  weeklyUsageUsd: 5,
  monthlyUsageUsd: 11,
  group: {
    id: 1,
    name: "Pro",
    dailyLimitUsd: 10,
    weeklyLimitUsd: 50,
    monthlyLimitUsd: 100,
    quota: 100,
  },
};

assertEqual(isActiveSubscription(sub), true);
assertEqual(isActiveSubscription({ ...sub, status: "expired" }), false);

assertDeepEqual(usageWindow(sub, null, "daily"), { used: 1.25, limit: 10, remaining: 8.75, percentage: undefined });
assertEqual(percentLabel(1.25, 10, null), "12.5%");
assertEqual(percentLabel(null, null, 66.666), "66.7%");
assertEqual(percentLabel(null, null, null), "-");
assertEqual(quotaLine(sub, null), "\u603b\u989d\u5ea6 $100.0000 / \u5269\u4f59 $89.0000");

const progressList: SubscriptionProgressInfo[] = [{
  subscription: sub,
  progress: {
    id: 7,
    monthly: { usedUsd: 20, limitUsd: 200, remainingUsd: 180, percentage: 10 },
  },
}];
const progress = subscriptionProgressInfo(sub, progressList);
assertEqual(progress?.id, 7);
assertDeepEqual(usageWindow(sub, progress, "monthly"), { used: 20, limit: 200, remaining: 180, percentage: 10 });
assertEqual(quotaLine({ ...sub, quota: null, remaining: null }, progress), "\u603b\u989d\u5ea6 $100.0000 / \u5269\u4f59 $180.0000");

console.log("subscription logic tests passed");
