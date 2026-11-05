-- scan: {data}/lineitem.parquet --columns l_orderkey --filter "l_shipmode = 'AIR'"
SELECT l_orderkey FROM '{data}/lineitem.parquet' WHERE l_shipmode = 'AIR'
