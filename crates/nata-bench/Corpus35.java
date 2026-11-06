import java.sql.*;
import java.nio.file.*;
import java.io.*;
public class Corpus35 {
 static String csv(String s) {if(s==null)return "NULL";return "\""+s.replace("\"","\"\"")+"\"";}
 public static void main(String[] args)throws Exception {
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/nata","test","")) {
   System.out.println("pgjdbc "+c.getMetaData().getDriverVersion()+" Java "+System.getProperty("java.version"));
   for(Path p:Files.list(Path.of(args[1])).sorted().toList()) {
    if(!p.toString().endsWith(".sql"))continue;
    String q=Files.readString(p).replace("{data}",args[2]);
    try(Statement s=c.createStatement();ResultSet r=s.executeQuery(q);
        PrintWriter out=new PrintWriter(Path.of(args[3],p.getFileName()+".csv").toFile())) {
     int n=r.getMetaData().getColumnCount();
     for(int i=1;i<=n;i++)out.print((i>1?",":"")+csv(r.getMetaData().getColumnLabel(i)));out.println();
     while(r.next()){for(int i=1;i<=n;i++)out.print((i>1?",":"")+csv(r.getString(i)));out.println();}
    } catch(SQLException e){System.out.println(p.getFileName()+" ERROR "+e.getSQLState());}
   }
  }
 }
}
