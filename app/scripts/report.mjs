// Pure helpers for the devnet run's reports (5.A rehearsal), tested by scripts/slate_check.mjs.
//
//   node scripts/report.mjs gate <phone-gate.json>   one line per screen, then the gate's summary
//
// - `parseGateReport`: phone_gate.mjs prints its JSON report and then a one-line summary, and
//   demo_devnet.sh keeps both in one file; a plain JSON.parse of that file fails.
// - `refusedDuplicate`: the crank key may also be held by another desk on the same cluster (the
//   rehearsal of 29 Oct ran next to an older desk from the main checkout). Both see a ticket's
//   last leg resolve and both send `settle`; the program refuses the second with TicketNotOpen.
//   They race on `finalize` too (7.2a-r): the second finds the Pending closed (3012).
//   Such a refused duplicate is harmless and is reported, not failed, when the same ticket has a
//   successful closing transaction in the run.

/// Anchor's 6000 + index of `SchedinaError::TicketNotOpen` (programs/schedina/src/error.rs).
export const TICKET_NOT_OPEN = 6032;
/// Anchor's `AccountNotInitialized`: a second `finalize` of a Pending the first one closed (the
/// 30 Oct rehearsal ran beside three desks of the user's with the same crank key).
export const ACCOUNT_NOT_INITIALIZED = 3012;

/// `{rows, summary}` from phone_gate.mjs's output: its JSON array, then any trailing text.
export function parseGateReport(text) {
  const start = text.indexOf("[");
  if (start < 0) throw new Error("no JSON report");
  // The report is `JSON.stringify(report, null, 1)`: it ends at the first line that is "]".
  const end = text.indexOf("\n]", start);
  const json = end < 0 ? text.slice(start) : text.slice(start, end + 2);
  const rows = JSON.parse(json);
  const summary = (end < 0 ? "" : text.slice(end + 2)).split("\n").map((l) => l.trim()).filter(Boolean);
  return { rows, summary };
}

const customCode = (err) => {
  const m = JSON.stringify(err ?? null).match(/"Custom":(\d+)/);
  return m ? Number(m[1]) : null;
};

/// Whether failed transaction `x` (an evidence row: sig, ix, ticket, signer, err) is a crank's
/// refused duplicate: signed by `crank`, a Settle refused with TicketNotOpen, on a ticket that
/// another transaction of `txs` closed successfully (Settle or CashOut).
export function refusedDuplicate(x, txs, crank) {
  if (x.err === null || x.err === undefined) return false;
  if (x.signer !== crank || !x.ticket) return false;
  const closedBy = (re) => txs.some((y) => y !== x && y.err === null && y.ticket === x.ticket && re.test(y.ix));
  if (/^Settle$/.test(x.ix) && customCode(x.err) === TICKET_NOT_OPEN) return closedBy(/^(Settle|CashOut)$/);
  // 7.2a-r: the desks also race on `finalize`; the loser finds the Pending already closed.
  if (/^Finalize$/.test(x.ix) && customCode(x.err) === ACCOUNT_NOT_INITIALIZED) return closedBy(/^Finalize$/);
  return false;
}

/// A builder fee as Confirm prints it (confirm.rs `usdc_fine`): USDC to the hundredth of a cent,
/// truncated, "0.0150 USDC". Since 6.1c it is no longer in euros (6.2a-r run 2 still expected
/// "€0.0150" and failed on a correct screen).
export function usdcFine(u) {
  const group = (n) => n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${group(Math.floor(u / 1e6))}.${String(Math.floor((u % 1e6) / 100)).padStart(4, "0")} USDC`;
}

/// Whether Confirm's fill list (`ul.fills` text) shows the total builder fee `u` on its
/// "Builder fee" line.
export function builderFeeShown(fillsText, u) {
  return new RegExp(`Builder fee\\s*${usdcFine(u).replace(/[.]/g, "\\.")}`).test(fillsText);
}

const { pathToFileURL } = await import("node:url");
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href && process.argv[2] === "gate") {
  const { readFileSync } = await import("node:fs");
  let parsed;
  try {
    parsed = parseGateReport(readFileSync(process.argv[3], "utf8"));
  } catch (e) {
    console.log("phone gate: unreadable report", String(e));
    process.exit(0);
  }
  for (const r of parsed.rows) {
    if (r.errors) console.log("phone gate", r.pass ?? r.scheme, "page errors:", r.errors);
    else if (r.scrollWidth !== undefined) {
      // 6.2a-g rows: pass (m390, d1024, …), name, width, problems / failedChecks.
      const why = [...(r.problems ?? []), ...(r.failedChecks ?? []), ...(r.banned?.length ? [`banned ${r.banned}`] : [])];
      console.log(`phone gate ${String(r.pass ?? r.scheme).padEnd(9)} ${r.name.padEnd(22)} scrollWidth ${r.scrollWidth} ${r.ok ? "ok" : `FAIL ${why.join("; ")}`}`);
    } else if (r.ok === false) {
      console.log(`phone gate ${String(r.pass ?? r.scheme).padEnd(9)} ${String(r.name).padEnd(22)} FAIL ${JSON.stringify(r.why ?? r.error ?? "")}`);
    }
  }
  for (const l of parsed.summary) console.log(l);
}
