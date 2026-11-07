import java.sql.*;
public class Metadata {
  static void consume(ResultSet r) throws Exception {long n=0; while(r.next()) {for(int i=1;i<=r.getMetaData().getColumnCount();i++)r.getObject(i);n++;} r.close(); System.out.println("rows="+n);}
  public static void main(String[] a) throws Exception {
    try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata?user=nata")) {
      System.out.println(c.getMetaData().getDriverVersion());
      if(a.length>1) {c.setAutoCommit(false); try(Statement s=c.createStatement()){s.setFetchSize(1000); try(ResultSet r=s.executeQuery("SELECT v FROM million")){long n=0;while(r.next())if(r.getLong(1)!=n++)throw new Exception("value"); if(n!=1000000)throw new Exception("count");}} c.rollback();return;}
      DatabaseMetaData m=c.getMetaData();
      consume(m.getCatalogs());consume(m.getSchemas());consume(m.getTables(null,null,"%",null));consume(m.getColumns(null,null,"%","%"));
      for(String schema:new String[]{"public","empty","Quoted Schema","other"}) {consume(m.getPrimaryKeys(null,schema,"all_types"));consume(m.getIndexInfo(null,schema,"all_types",false,false));}
      consume(m.getTypeInfo());consume(m.getFunctions(null,null,"%"));
      org.postgresql.PGConnection pg=c.unwrap(org.postgresql.PGConnection.class);
      pg.getCopyAPI().copyOut("COPY public.all_types TO STDOUT WITH (FORMAT csv, HEADER true)",java.io.OutputStream.nullOutputStream());
    }
  }
}
