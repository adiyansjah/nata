SELECT id, CASE i WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'many' END AS a FROM '{data}/sqlcheck/types.parquet'
