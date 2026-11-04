select l_shipmode,
       sum(case when o_orderpriority in ('1-URGENT', '2-HIGH') then 1 else 0 end),
       sum(case when o_orderpriority not in ('1-URGENT', '2-HIGH') then 1 else 0 end)
from orders, lineitem
where o_orderkey = l_orderkey
  and l_shipmode in ('MAIL', 'SHIP')
  and l_commitdate < l_receiptdate
  and l_shipdate < l_commitdate
  and l_receiptdate >= date '1994-01-01'
  and l_receiptdate < date '1995-01-01'
group by l_shipmode;
