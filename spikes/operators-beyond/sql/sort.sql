select l_orderkey, l_linenumber, l_shipdate, l_extendedprice, l_comment from lineitem order by l_shipdate desc nulls first, l_orderkey, l_linenumber
