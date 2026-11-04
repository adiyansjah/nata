select l_shipmode, l_shipdate - date '1970-01-01', count(*), sum(l_quantity), sum(l_extendedprice)
from lineitem
where l_discount between 0.02 and 0.08
group by l_shipmode, l_shipdate;
