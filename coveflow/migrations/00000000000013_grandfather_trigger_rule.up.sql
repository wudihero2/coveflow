-- Grandfather existing flow definitions to the legacy fan-in behaviour.
--
-- The fan-in default changed: the old engine hard-coded "no upstream failed AND
-- >=1 active incoming edge" (== TriggerRule::NoneFailedMinOneSuccess) for every
-- node. The new model adds an explicit per-node `trigger_rule` whose serde
-- default is the stricter `all_success` (any inactive/skipped incoming edge ->
-- the node is Skipped). Without this migration, every saved flow with a join
-- downstream of a conditional/branch edge would silently change behaviour (the
-- join, and everything below it, would now be skipped) — the absent field being
-- defaulted hides the change.
--
-- Fix: stamp every existing node that has no explicit `trigger_rule` with the
-- legacy default, so saved flows keep running exactly as authored. New flows
-- created after this migration keep the `all_success` default. This applies to
-- ALL such nodes (not just those with conditional incoming edges): a branch join
-- `branch -(case)-> b/c -> d` carries its case on b/c's *incoming* edges, so d's
-- own edges are plain — only blanket-stamping preserves d's behaviour.

UPDATE flow f
SET value = jsonb_set(
    f.value,
    '{nodes}',
    (
        SELECT jsonb_agg(
            CASE WHEN n ? 'trigger_rule'
                 THEN n
                 ELSE n || '{"trigger_rule":"none_failed_min_one_success"}'::jsonb
            END
            ORDER BY ord
        )
        FROM jsonb_array_elements(f.value -> 'nodes') WITH ORDINALITY AS t(n, ord)
    )
)
WHERE jsonb_typeof(f.value -> 'nodes') = 'array'
  AND EXISTS (
      SELECT 1 FROM jsonb_array_elements(f.value -> 'nodes') x
      WHERE NOT (x ? 'trigger_rule')
  );

-- Also pin in-flight / queued flow runs (their flow_value snapshot drives the
-- engine). Completed runs are historical and left untouched.
UPDATE run r
SET flow_value = jsonb_set(
    r.flow_value,
    '{nodes}',
    (
        SELECT jsonb_agg(
            CASE WHEN n ? 'trigger_rule'
                 THEN n
                 ELSE n || '{"trigger_rule":"none_failed_min_one_success"}'::jsonb
            END
            ORDER BY ord
        )
        FROM jsonb_array_elements(r.flow_value -> 'nodes') WITH ORDINALITY AS t(n, ord)
    )
)
WHERE r.kind IN ('flow', 'flow_preview')
  AND r.flow_value IS NOT NULL
  AND jsonb_typeof(r.flow_value -> 'nodes') = 'array'
  AND NOT EXISTS (SELECT 1 FROM run_completed c WHERE c.id = r.id)
  AND EXISTS (
      SELECT 1 FROM jsonb_array_elements(r.flow_value -> 'nodes') x
      WHERE NOT (x ? 'trigger_rule')
  );
