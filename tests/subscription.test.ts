import { buildAccountAlerts, isActiveSubscription, money, quotaLine, usageWindow, type SubscriptionProgressInfo, type SubscriptionSummary } from "../src/subscription";

import { activeSubscriptionGroupIds, prioritizeByActiveSubscription, recommendedSubscriptionGroupId, subscriptionsWithResolvedGroups, type GroupSummary } from "../src/subscription";

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
assert(!isActiveSubscription({
  id: 8,
  status: "active",
  groupId: 88,
  remaining: 0,
}), "exhausted subscription is not eligible");
assert(money(1.2) === "$1.2000", "money format");
const progressValue = progress[0].progress ?? null;
assert(quotaLine(sub, progressValue).includes("\u5269\u4f59"), "quota line");
const daily = usageWindow(sub, progressValue, "daily");
assert(daily.used === 9, "daily used");
const alerts = buildAccountAlerts({ balance: 0.5, subscriptions: [sub], subscriptionProgress: progress });
assert(alerts.some((item) => item.title.includes("\u4f59\u989d")), "balance alert");
assert(alerts.some((item) => item.title.includes("\u5230\u671f") || item.title.includes("\u989d\u5ea6")), "expiry/usage alert");
const subscriptions: SubscriptionSummary[] = [
  { id: 2, status: "active", groupId: 22, expiresAt: new Date(Date.now() + 60 * 24 * 60 * 60 * 1000).toISOString() },
  { id: 3, status: "active", groupId: 11, expiresAt: new Date(Date.now() + 6 * 24 * 60 * 60 * 1000).toISOString() },
  { id: 4, status: "expired", groupId: 33, expiresAt: "2026-07-01T00:00:00Z" },
];
assert(activeSubscriptionGroupIds(subscriptions).join(",") === "11,22", "active subscriptions ordered by expiry");
assert(recommendedSubscriptionGroupId(subscriptions) === 11, "soonest active subscription is recommended");

const keys = [
  { id: "balance", groupId: null as number | null },
  { id: "later-subscription", groupId: 22 as number | null },
  { id: "expired-subscription", groupId: 33 as number | null },
  { id: "soon-subscription", groupId: 11 as number | null },
];
const prioritized = prioritizeByActiveSubscription(keys, subscriptions, (item) => item.groupId);
assert(prioritized.map((item) => item.id).join(",") === "soon-subscription,later-subscription,balance,expired-subscription", "subscription keys precede balance keys");
const unchanged = prioritizeByActiveSubscription(keys, [], (item) => item.groupId);
assert(unchanged.map((item) => item.id).join(",") === keys.map((item) => item.id).join(","), "balance order remains stable without active subscriptions");
const mixedPlatform: SubscriptionSummary[] = [
  { id: 5, status: "active", groupId: 55, group: { id: 55, name: "Claude", platform: "anthropic" } },
  { id: 6, status: "active", groupId: 66, group: { id: 66, name: "Codex", platform: "openai" } },
  { id: 7, status: "active", groupId: 77, startsAt: "2099-01-01T00:00:00Z" },
];
assert(activeSubscriptionGroupIds(mixedPlatform).join(",") === "66", "only current OpenAI subscriptions are eligible for Codex");

const nameOnlySubscription: SubscriptionSummary = { id: 9, status: "active", groupName: "Balance" };
const availableGroups: GroupSummary[] = [
  { id: 10, name: "Balance", platform: "openai" },
  { id: 11, name: "2000次卡", platform: "openai" },
];
const resolvedByProgressName = subscriptionsWithResolvedGroups([nameOnlySubscription], availableGroups, [{
  subscription: nameOnlySubscription,
  progress: {
    id: 9,
    groupName: " 2000次卡 ",
    daily: { remainingUsd: 0 },
    weekly: { remainingUsd: 20 },
    monthly: { remainingUsd: 100 },
  },
}]);
assert(resolvedByProgressName[0].group?.id === 11, "progress group name resolves the subscription group before stale summary names");
assert(resolvedByProgressName[0].groupId === 11, "resolved group id is available for key selection");
assert(resolvedByProgressName[0].remaining === 100, "positive usage windows keep the subscription available");
assert(activeSubscriptionGroupIds(resolvedByProgressName).join(",") === "11", "one exhausted usage window does not invalidate the subscription");

const aggregateRemainingSubscription: SubscriptionSummary = { id: 10, status: "active", groupId: 11, remaining: 25 };
const allWindowsExhausted = subscriptionsWithResolvedGroups([aggregateRemainingSubscription], availableGroups, [{
  subscription: aggregateRemainingSubscription,
  progress: { id: 10, daily: { remainingUsd: 0 }, weekly: { remainingUsd: 0 }, monthly: { remainingUsd: 0 } },
}]);
assert(allWindowsExhausted[0].remaining === 0, "all reported usage windows derive exhausted remaining");
assert(!isActiveSubscription(allWindowsExhausted[0]), "all exhausted usage windows make the subscription unavailable");

const aggregateWithoutProgress = subscriptionsWithResolvedGroups([aggregateRemainingSubscription], availableGroups);
assert(aggregateWithoutProgress[0].remaining === 25, "server aggregate remaining is preserved without usage window data");
console.log("subscription logic tests passed");
