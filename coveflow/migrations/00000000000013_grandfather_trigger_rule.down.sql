-- Best-effort reverse: strip the legacy default this migration stamped. We can't
-- distinguish migration-stamped from user-set `none_failed_min_one_success`, so
-- this removes all of them (acceptable for the dev-only rollback this targets).

UPDATE flow f
SET value = jsonb_set(
    f.value,
    '{nodes}',
    (
        SELECT jsonb_agg(
            CASE WHEN n ->> 'trigger_rule' = 'none_failed_min_one_success'
                 THEN n - 'trigger_rule'
                 ELSE n
            END
            ORDER BY ord
        )
        FROM jsonb_array_elements(f.value -> 'nodes') WITH ORDINALITY AS t(n, ord)
    )
)
WHERE jsonb_typeof(f.value -> 'nodes') = 'array'
  AND EXISTS (
      SELECT 1 FROM jsonb_array_elements(f.value -> 'nodes') x
      WHERE x ->> 'trigger_rule' = 'none_failed_min_one_success'
  );

UPDATE run r
SET flow_value = jsonb_set(
    r.flow_value,
    '{nodes}',
    (
        SELECT jsonb_agg(
            CASE WHEN n ->> 'trigger_rule' = 'none_failed_min_one_success'
                 THEN n - 'trigger_rule'
                 ELSE n
            END
            ORDER BY ord
        )
        FROM jsonb_array_elements(r.flow_value -> 'nodes') WITH ORDINALITY AS t(n, ord)
    )
)
WHERE r.kind IN ('flow', 'flow_preview')
  AND r.flow_value IS NOT NULL
  AND jsonb_typeof(r.flow_value -> 'nodes') = 'array'
  AND EXISTS (
      SELECT 1 FROM jsonb_array_elements(r.flow_value -> 'nodes') x
      WHERE x ->> 'trigger_rule' = 'none_failed_min_one_success'
  );
