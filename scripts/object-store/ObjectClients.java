import java.sql.*;

/** pgjdbc with its default settings against `nata serve`: a registered object-store table and the same table read locally must answer alike. */
public class ObjectClients {
  static String rows(Connection c, String sql, boolean prepared) throws Exception {
    StringBuilder b = new StringBuilder();
    try (PreparedStatement s = c.prepareStatement(sql)) {
      for (int run = 0; run < (prepared ? 3 : 1); run++) {
        b.setLength(0);
        try (ResultSet r = s.executeQuery()) {
          int n = r.getMetaData().getColumnCount();
          while (r.next()) { for (int i = 1; i <= n; i++) b.append(r.getString(i)).append('|'); b.append('\n'); }
        }
      }
    }
    return b.toString();
  }

  static String columns(Connection c, String table) {
    try (ResultSet r = c.getMetaData().getColumns(null, null, table, "l_orderkey")) {
      int n = 0;
      while (r.next()) n++;
      return "ok rows=" + n;
    } catch (SQLException e) {
      return "error " + e.getSQLState();
    }
  }

  public static void main(String[] a) throws Exception {
    int failures = 0;
    try (Connection c = DriverManager.getConnection("jdbc:postgresql://127.0.0.1:" + a[0] + "/nata?user=nata")) {
      String[] queries = {
        "SELECT count(*), sum(l_quantity) FROM nata.public.%s",
        "SELECT l_orderkey, l_shipdate, l_comment FROM nata.public.%s WHERE l_orderkey < 40 LIMIT 25",
        "SELECT l_returnflag, count(*) FROM nata.public.%s GROUP BY l_returnflag",
      };
      for (String q : queries) {
        String remote = rows(c, String.format(q, "remote_table"), true);
        String local = rows(c, String.format(q, "local_table"), true);
        boolean same = q.contains("GROUP BY")
            ? java.util.Arrays.stream(remote.split("\n")).sorted().toList().equals(java.util.Arrays.stream(local.split("\n")).sorted().toList())
            : remote.equals(local);
        System.out.println("pgjdbc " + (same && !remote.isEmpty() ? "PASS" : "FAIL") + ": " + q.substring(0, Math.min(60, q.length())));
        if (!same || remote.isEmpty()) failures++;
      }
      // metadata: the object-store table must behave exactly like its local twin. (Catalog views are track C, card 0007: on 2026-11-07 pgjdbc's getColumns
      // still ends in 0A000 "captured catalog helper outside the skeleton metadata contract" for BOTH; what this checks is that the location makes no difference.)
      String remoteMeta = columns(c, "remote_table"), localMeta = columns(c, "local_table");
      boolean same = remoteMeta.equals(localMeta);
      System.out.println("pgjdbc " + (same ? "PASS" : "FAIL") + ": getColumns of the object-store table is the local table's (" + remoteMeta + ")");
      if (!same) failures++;
    }
    System.exit(failures == 0 ? 0 : 1);
  }
}
