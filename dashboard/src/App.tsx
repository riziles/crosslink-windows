// Minimal placeholder App for the dashboard scaffolding (GH #429, P1.1).
//
// This is the walking skeleton — it proves the frontend builds cleanly
// and the bundled assets serve from the `crosslink dashboard` binary.
// Real routing, state, and pages land in subsequent P1.* subissues of
// #689. See DESIGN-CROSSLINK-DASHBOARD.md §8 for the planned IA.

export function App() {
  return (
    <main className="min-h-screen bg-background text-foreground">
      <div className="mx-auto flex min-h-screen max-w-3xl flex-col items-start justify-center gap-4 px-6 py-24">
        <h1 className="text-3xl font-semibold">crosslink dashboard</h1>
        <p className="text-muted-foreground">
          The dashboard scaffolding is in place. Project tracking, alert
          streams, and the SCADA tile grid will arrive in subsequent
          commits against the{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-xs">
            feat/429-crosslink-dashboard
          </code>{" "}
          branch.
        </p>
        <p className="text-sm text-muted-foreground">
          See{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-xs">
            DESIGN-CROSSLINK-DASHBOARD.md
          </code>{" "}
          at the repo root for the design, and GH #429 for the
          originating issue.
        </p>
      </div>
    </main>
  );
}
