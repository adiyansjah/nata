# Reference-only sizing: DROP SCHEMA locks every table in one transaction.
resource_tables=(64 1024 10000)
resource_max_tables=0
for resource_count in "${resource_tables[@]}"; do
    (( resource_count <= resource_max_tables )) || resource_max_tables=$resource_count
done
# Two lock slots per table plus 64 for schema/catalog overhead. These fixtures
# have bigint columns only (no indexes/TOAST); PostgreSQL also pools slots across
# max_connections, providing further headroom. This changes no Nata limit.
resource_pg_locks=$((2 * resource_max_tables + 64))
