-- lineitem-shaped table for the generic-ops engine (spikes/generic-ops: its Q1 and Q6 are written against these
-- column names and types). NULL probability @P@ on the three decimal columns, half of it on l_shipdate; the group keys
-- l_returnflag and l_linestatus are NULL with probability @PK@ (0 except in the li_nullkey set, where the generic Q1
-- sink must refuse loudly: "NULL group key: not supported yet").
COPY (
  SELECT i AS l_id,
         CASE WHEN nul(i, 11, @PK@) THEN NULL ELSE substr('ANR', 1 + ri(i, 1, 3)::INTEGER, 1) END AS l_returnflag,
         CASE WHEN nul(i, 12, @PK@) THEN NULL ELSE substr('FO', 1 + ri(i, 2, 2)::INTEGER, 1) END AS l_linestatus,
         CASE WHEN nul(i, 3, @P@) THEN NULL ELSE (1 + ri(i, 4, 50))::DECIMAL(15,2) END AS l_quantity,
         CASE WHEN nul(i, 5, @P@) THEN NULL ELSE ((90000 + ri(i, 6, 10410001)) * 0.01)::DECIMAL(15,2) END AS l_extendedprice,
         CASE WHEN nul(i, 7, @P@) THEN NULL ELSE (ri(i, 8, 11) * 0.01)::DECIMAL(15,2) END AS l_discount,
         CASE WHEN nul(i, 9, (@P@) / 2) THEN NULL ELSE (DATE '1992-01-01' + ri(i, 10, 2557)::INTEGER) END AS l_shipdate
  FROM range(@NL@) t(i)
) TO '@DIR@/li_@NAME@.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
