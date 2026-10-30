import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

/** Stand-in for the Minecraft client: records its arguments and exits. */
public class Main {
    public static void main(String[] args) throws Exception {
        StringBuilder sb = new StringBuilder();
        for (String s : args) sb.append(s).append('\n');
        sb.append("java.library.path=").append(System.getProperty("java.library.path")).append('\n');
        sb.append("cwd=").append(Paths.get("").toAbsolutePath()).append('\n');
        sb.append("mock.lib=").append(System.getProperty("mock.lib", "")).append('\n');
        Files.write(Paths.get("launch_args.txt"), sb.toString().getBytes(StandardCharsets.UTF_8));
        System.out.println("MOCK MINECRAFT OK");
    }
}
