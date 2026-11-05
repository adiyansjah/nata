-- scan: {data}/lineitem.parquet --columns l_returnflag,l_linestatus,l_discount --filter "l_discount >= 0.055" --filter "l_returnflag = 'R'"
SELECT l_returnflag, l_linestatus, l_discount
FROM '{data}/lineitem.parquet'
WHERE l_discount >= 0.055 AND l_returnflag = 'R'
