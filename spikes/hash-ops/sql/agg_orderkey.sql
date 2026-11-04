select l_orderkey, count(*), sum(l_extendedprice), sum(l_extendedprice * (1 - l_discount))
from lineitem
where l_shipdate <= date '1998-09-02'
group by l_orderkey;
