-- (3) MANY NULLs, one file per NULL percentage @PP@ (probability @P@ for every column independently):
-- keys AND values. kn BIGINT, ki INTEGER, kd DATE and ks VARCHAR are group/join keys; v and w are SUM inputs.
-- Non-NULL ks values include '' (about 5 percent of them), so '' and NULL are both present.
COPY (
  SELECT i AS id,
         CASE WHEN nul(i, 1, @P@) THEN NULL ELSE ri(i, 2, @K@)::BIGINT END AS kn,
         CASE WHEN nul(i, 3, @P@) THEN NULL ELSE ri(i, 4, 5000)::INTEGER END AS ki,
         CASE WHEN nul(i, 5, @P@) THEN NULL ELSE (DATE '2000-01-01' + ri(i, 6, 3000)::INTEGER) END AS kd,
         CASE WHEN nul(i, 7, @P@) THEN NULL
              WHEN u01(i, 8) < 0.05 THEN ''
              ELSE 'ks' || ri(i, 9, 20000)::VARCHAR END AS ks,
         CASE WHEN nul(i, 10, @P@) THEN NULL ELSE ri(i, 11, 1000) END AS v,
         CASE WHEN nul(i, 12, @P@) THEN NULL ELSE ri(i, 13, 1000001) - 500000 END AS w
  FROM range(@NP@) t(i)
) TO '@DIR@/nul_p@PP@.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
