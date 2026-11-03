// L7-1 visual shots for 7.1f (l7-contracts L7-4): Account (31 Oct: "Your wallet", "Faucet",
// then "Your bets", L8-4) at 390 px, its first screen and the full page (the bets scrolled to are
// my-bets.mjs's `account-bets-390`), and the two pages that left it, /house and /profile. No
// mockup: compared with the drawn screens' style (the tipster header, the cards). L6's desktop
// composition shots are gone with that layout (V-7, D-440: one centred 480 px column; the shell
// shot `shell-1440` covers it).
export const shots = () => [
  { name: "account-top-390", path: "/account", wait: "#bets", vp: "m390", full: false, mockup: ["profile", "sportsbook"] },
  { name: "account-390", path: "/account", wait: "#bets .mb-card", vp: "m390", full: true, mockup: ["profile", "sportsbook"] },
  { name: "house-390", path: "/house", wait: "#account-house #house-hedger", vp: "m390", full: false, mockup: ["profile"] },
  { name: "profile-edit-390", path: "/profile", wait: "#account-profile", vp: "m390", full: false, mockup: ["profile"] },
];
