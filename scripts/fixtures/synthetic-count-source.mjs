// A stand-in count source for the Session Tracker's tests, swapped in for count-source.ts by
// scripts/session-tracker-harness.mjs. It counts the simplest way there is, so tests can say
// exactly what was drunk:
// - HpMpChanged: a rise of exactly k x 160 HP since the last reading is k Meat, of k x 150 MP is
//   k Lemon, when the reading is continuous.
// - { type: "TestMaybe", item, count }: that many maybe uses of the potion.
// - { type: "TestHealing", resource, on }: heals from others on or off for "hp" or "mp".
// - { type: "TestSlot", item, count }: that many uses of the potion counted from its quick slot.
// - { type: "TestAmounts", item, amounts }: the amounts the potion is counted at from now on.

const MEAT = 2010003;
const LEMON = 2010005;

export function wiredSource() {
  let last = null;
  const healing = { hp: false, mp: false };
  const amounts = { [MEAT]: [160], [LEMON]: [150] };
  return {
    onEvent(event) {
      if (event.type === "TestMaybe") return [{ item: event.item, count: event.count, maybe: true, source: "rises" }];
      if (event.type === "TestSlot") return [{ item: event.item, count: event.count, maybe: false, source: "slots" }];
      if (event.type === "TestAmounts") {
        amounts[event.item] = event.amounts;
        return [];
      }
      if (event.type === "TestHealing") {
        healing[event.resource] = event.on;
        return [];
      }
      if (event.type !== "HpMpChanged") return [];
      const before = last;
      last = event;
      if (before === null || !event.continuous) return [];
      const uses = [];
      const hp = event.hp - before.hp;
      const mp = event.mp - before.mp;
      if (hp > 0 && hp % 160 === 0) uses.push({ item: MEAT, count: hp / 160, maybe: false, source: "rises" });
      if (mp > 0 && mp % 150 === 0) uses.push({ item: LEMON, count: mp / 150, maybe: false, source: "rises" });
      return uses;
    },
    healing: (resource) => healing[resource],
    amounts: (item) => amounts[item] ?? [],
  };
}
