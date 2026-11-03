SELECT l_orderkey, l_linenumber, l_quantity
FROM '{data}/lineitem.parquet'
WHERE l_quantity > 49
ORDER BY l_orderkey, l_linenumber
LIMIT 20
