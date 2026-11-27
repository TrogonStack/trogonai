import java.net.HttpURLConnection;
import java.net.URI;

public class ProxyProbe {
  public static void main(String[] args) throws Exception {
    HttpURLConnection connection = (HttpURLConnection) new URI(args[0]).toURL().openConnection();
    connection.setConnectTimeout(5000);
    connection.setReadTimeout(5000);
    System.out.println("code=" + connection.getResponseCode());
  }
}
