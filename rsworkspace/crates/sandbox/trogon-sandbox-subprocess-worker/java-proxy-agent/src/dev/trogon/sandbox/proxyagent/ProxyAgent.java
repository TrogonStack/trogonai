package dev.trogon.sandbox.proxyagent;

import java.lang.instrument.Instrumentation;
import java.lang.reflect.Method;
import java.net.Authenticator;
import java.net.InetAddress;
import java.net.PasswordAuthentication;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Properties;
import java.util.Set;

/**
 * Makes a JVM in the sandbox use the lease's egress proxy.
 *
 * <p>The JVM ignores HTTPS_PROXY and NO_PROXY, and a proxy credential can only reach
 * HttpURLConnection and java.net.http through an {@link Authenticator}. Both need code running
 * inside the JVM, so the worker injects this agent with {@code -javaagent} and keeps the
 * credential in the environment, where the agent reads it. Nothing secret appears in the
 * JAVA_TOOL_OPTIONS value the JVM echoes on stderr.
 *
 * <p>From the proxy variables it sets http[s].proxyHost and proxyPort and http.nonProxyHosts
 * (each unless given as an explicit -D argument), clears the JDK's block on Basic for CONNECT
 * tunnels, and installs a default Authenticator that answers only for the proxy endpoint. It is
 * dependency free and compiled for Java 8.
 */
public final class ProxyAgent {

  private ProxyAgent() {}

  public static void premain(String args, Instrumentation instrumentation) {
    install();
  }

  public static void agentmain(String args, Instrumentation instrumentation) {
    install();
  }

  static void install() {
    try {
      configure(
          env("HTTPS_PROXY", "https_proxy"),
          env("HTTP_PROXY", "http_proxy"),
          env("NO_PROXY", "no_proxy"));
    } catch (RuntimeException e) {
      System.err.println("[sandbox-proxy-agent] ignoring: " + e);
    }
  }

  private static String env(String upper, String lower) {
    String value = System.getenv(upper);
    return value != null && !value.isEmpty() ? value : System.getenv(lower);
  }

  static void configure(String httpsProxy, String httpProxy, String noProxy) {
    Endpoint https = Endpoint.parse(httpsProxy);
    Endpoint http = Endpoint.parse(httpProxy);
    if (https == null && http == null) {
      return;
    }
    Set<String> explicit = explicitProperties();
    Properties props = System.getProperties();
    if (https != null) {
      set(props, explicit, "https.proxyHost", https.host);
      set(props, explicit, "https.proxyPort", Integer.toString(https.port));
    }
    if (http != null) {
      set(props, explicit, "http.proxyHost", http.host);
      set(props, explicit, "http.proxyPort", Integer.toString(http.port));
    }
    String nonProxyHosts = nonProxyHosts(noProxy);
    if (nonProxyHosts != null) {
      set(props, explicit, "http.nonProxyHosts", nonProxyHosts);
    }
    set(props, explicit, "jdk.http.auth.tunneling.disabledSchemes", "");

    List<Endpoint> credentialed = new ArrayList<Endpoint>();
    if (https != null && https.user != null) {
      credentialed.add(https);
    }
    if (http != null && http.user != null && !http.sameAs(https)) {
      credentialed.add(http);
    }
    if (!credentialed.isEmpty()) {
      Authenticator.setDefault(new ProxyAuthenticator(credentialed, previousAuthenticator()));
    }
  }

  private static void set(Properties props, Set<String> explicit, String key, String value) {
    if (!explicit.contains(key)) {
      props.setProperty(key, value);
    }
  }

  /** Names passed as -Dname=value, which the JVM seeds itself otherwise (macOS system proxy). */
  static Set<String> explicitProperties() {
    Set<String> names = new HashSet<String>();
    try {
      Object runtime =
          Class.forName("java.lang.management.ManagementFactory")
              .getMethod("getRuntimeMXBean")
              .invoke(null);
      @SuppressWarnings("unchecked")
      List<String> arguments =
          (List<String>)
              Class.forName("java.lang.management.RuntimeMXBean")
                  .getMethod("getInputArguments")
                  .invoke(runtime);
      for (String argument : arguments) {
        if (argument.startsWith("-D")) {
          int equals = argument.indexOf('=');
          names.add(equals < 0 ? argument.substring(2) : argument.substring(2, equals));
        }
      }
    } catch (ReflectiveOperationException e) {
      return names;
    } catch (RuntimeException e) {
      return names;
    }
    return names;
  }

  private static Authenticator previousAuthenticator() {
    try {
      return (Authenticator) Authenticator.class.getMethod("getDefault").invoke(null);
    } catch (ReflectiveOperationException e) {
      return null;
    } catch (RuntimeException e) {
      return null;
    }
  }

  /** Answers for the proxy endpoint only, and hands every other request to the previous one. */
  static final class ProxyAuthenticator extends Authenticator {
    private final List<Endpoint> endpoints;
    private final Authenticator previous;

    ProxyAuthenticator(List<Endpoint> endpoints, Authenticator previous) {
      this.endpoints = endpoints;
      this.previous = previous;
    }

    @Override
    protected PasswordAuthentication getPasswordAuthentication() {
      String host = getRequestingHost();
      if (host != null) {
        for (Endpoint endpoint : endpoints) {
          if (endpoint.port == getRequestingPort() && endpoint.host.equalsIgnoreCase(host)) {
            return new PasswordAuthentication(endpoint.user, endpoint.password.toCharArray());
          }
        }
      }
      if (previous == null) {
        return null;
      }
      try {
        Method delegate =
            Authenticator.class.getMethod(
                "requestPasswordAuthenticationInstance",
                String.class,
                InetAddress.class,
                int.class,
                String.class,
                String.class,
                String.class,
                URL.class,
                RequestorType.class);
        return (PasswordAuthentication)
            delegate.invoke(
                previous,
                host,
                getRequestingSite(),
                getRequestingPort(),
                getRequestingProtocol(),
                getRequestingPrompt(),
                getRequestingScheme(),
                getRequestingURL(),
                getRequestorType());
      } catch (ReflectiveOperationException e) {
        return null;
      }
    }
  }

  /** {@code http://[user[:password]@]host[:port]}; other schemes are not CONNECT proxies. */
  static final class Endpoint {
    final String host;
    final int port;
    final String user;
    final String password;

    private Endpoint(String host, int port, String user, String password) {
      this.host = host;
      this.port = port;
      this.user = user;
      this.password = password;
    }

    boolean sameAs(Endpoint other) {
      return other != null && other.port == port && other.host.equalsIgnoreCase(host);
    }

    static Endpoint parse(String url) {
      if (url == null) {
        return null;
      }
      String rest = url.trim();
      String scheme = "http";
      int separator = rest.indexOf("://");
      if (separator >= 0) {
        scheme = rest.substring(0, separator).toLowerCase(Locale.ROOT);
        rest = rest.substring(separator + 3);
      }
      if (!scheme.equals("http") && !scheme.equals("https")) {
        return null;
      }
      int slash = rest.indexOf('/');
      if (slash >= 0) {
        rest = rest.substring(0, slash);
      }
      String user = null;
      String password = "";
      int at = rest.lastIndexOf('@');
      if (at >= 0) {
        String userinfo = rest.substring(0, at);
        rest = rest.substring(at + 1);
        int colon = userinfo.indexOf(':');
        user = decode(colon < 0 ? userinfo : userinfo.substring(0, colon));
        if (colon >= 0) {
          password = decode(userinfo.substring(colon + 1));
        }
      }
      String host = rest;
      int port = scheme.equals("https") ? 443 : 80;
      if (rest.startsWith("[")) {
        int close = rest.indexOf(']');
        if (close < 0) {
          return null;
        }
        host = rest.substring(1, close);
        String tail = rest.substring(close + 1);
        if (tail.startsWith(":")) {
          port = number(tail.substring(1));
        } else if (!tail.isEmpty()) {
          return null;
        }
      } else {
        int colon = rest.lastIndexOf(':');
        if (colon >= 0) {
          host = rest.substring(0, colon);
          port = number(rest.substring(colon + 1));
        }
      }
      if (host.isEmpty() || port <= 0 || port > 65535) {
        return null;
      }
      return new Endpoint(host, port, user, password);
    }

    private static int number(String text) {
      try {
        return Integer.parseInt(text);
      } catch (NumberFormatException e) {
        return -1;
      }
    }
  }

  /** Percent-decoding for userinfo; unlike URLDecoder, a plus sign stays a plus sign. */
  static String decode(String text) {
    if (text.indexOf('%') < 0) {
      return text;
    }
    byte[] in = text.getBytes(StandardCharsets.UTF_8);
    byte[] out = new byte[in.length];
    int length = 0;
    for (int i = 0; i < in.length; i++) {
      if (in[i] == '%' && i + 2 < in.length) {
        int high = Character.digit(in[i + 1], 16);
        int low = Character.digit(in[i + 2], 16);
        if (high >= 0 && low >= 0) {
          out[length++] = (byte) ((high << 4) | low);
          i += 2;
          continue;
        }
      }
      out[length++] = in[i];
    }
    return new String(out, 0, length, StandardCharsets.UTF_8);
  }

  /**
   * NO_PROXY as http.nonProxyHosts: bar separated, wildcard only at an edge. Setting the property
   * replaces the JDK's loopback defaults, so they are added back. IPv4 ranges on a byte boundary
   * become dotted prefixes; entries the syntax cannot express are dropped rather than
   * mistranslated. Null when there is nothing to set.
   */
  static String nonProxyHosts(String noProxy) {
    if (noProxy == null || noProxy.trim().isEmpty()) {
      return null;
    }
    List<String> hosts = new ArrayList<String>();
    for (String defaultHost : new String[] {"localhost", "127.*", "[::1]", "0.0.0.0", "[::0]"}) {
      addOnce(hosts, defaultHost);
    }
    for (String raw : noProxy.split(",")) {
      String entry = raw.trim();
      if (entry.isEmpty()) {
        continue;
      }
      if (entry.equals("*")) {
        return "*";
      }
      int slash = entry.indexOf('/');
      if (slash >= 0) {
        String prefix = cidrPrefix(entry.substring(0, slash), entry.substring(slash + 1));
        if (prefix != null) {
          addOnce(hosts, prefix);
        }
      } else if (entry.startsWith(".")) {
        addOnce(hosts, "*" + entry);
      } else if (entry.indexOf(':') >= 0 && !entry.startsWith("[")) {
        addOnce(hosts, entry.indexOf(':') == entry.lastIndexOf(':')
            ? entry.substring(0, entry.indexOf(':'))
            : "[" + entry + "]");
      } else {
        addOnce(hosts, entry);
      }
    }
    StringBuilder joined = new StringBuilder();
    for (String host : hosts) {
      if (joined.length() > 0) {
        joined.append('|');
      }
      joined.append(host);
    }
    return joined.toString();
  }

  private static void addOnce(List<String> hosts, String host) {
    if (!hosts.contains(host)) {
      hosts.add(host);
    }
  }

  /** An IPv4 range on a byte boundary as a dotted prefix such as {@code 10.*}; else null. */
  static String cidrPrefix(String address, String bits) {
    String[] octets = address.split("\\.");
    int length;
    try {
      length = Integer.parseInt(bits);
    } catch (NumberFormatException e) {
      return null;
    }
    if (octets.length != 4 || length <= 0 || length > 32 || length % 8 != 0) {
      return null;
    }
    StringBuilder prefix = new StringBuilder();
    for (int i = 0; i < length / 8; i++) {
      prefix.append(octets[i]).append(i < 3 ? "." : "");
    }
    return length == 32 ? prefix.toString() : prefix + "*";
  }
}
