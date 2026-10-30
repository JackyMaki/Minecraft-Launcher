import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

/** Stand-in for the Forge/NeoForge installer: `--installClient <dir>`. */
public class Installer {
    public static void main(String[] args) throws Exception {
        if (args.length != 2 || !args[0].equals("--installClient")) {
            System.err.println("usage: --installClient <dir>");
            System.exit(2);
        }
        File root = new File(args[1]);
        if (!new File(root, "launcher_profiles.json").isFile()) {
            System.err.println("There is no minecraft launcher profile in the target directory");
            System.exit(1);
        }
        String id = System.getProperty("mock.installer.id", "mock-1.21-forge-52.0.1");
        File dir = new File(new File(root, "versions"), id);
        dir.mkdirs();
        String json = "{\"id\":\"" + id + "\",\"inheritsFrom\":\"mock-1.21\",\"type\":\"release\",\"mainClass\":\"Main\","
            + "\"arguments\":{\"game\":[\"--forgeMarker\"],\"jvm\":[\"-Dmock.forge=true\"]},"
            + "\"libraries\":[{\"name\":\"net.mockforge:forge:52.0.1:client\",\"downloads\":{\"artifact\":{"
            + "\"path\":\"net/mockforge/forge/52.0.1/forge-52.0.1-client.jar\",\"url\":\"\",\"sha1\":\"\",\"size\":0}}}]}";
        Files.write(new File(dir, id + ".json").toPath(), json.getBytes(StandardCharsets.UTF_8));
        File lib = new File(root, "libraries/net/mockforge/forge/52.0.1/forge-52.0.1-client.jar");
        lib.getParentFile().mkdirs();
        try (ZipOutputStream z = new ZipOutputStream(new FileOutputStream(lib))) {
            z.putNextEntry(new ZipEntry("forge.txt"));
            z.write("patched".getBytes(StandardCharsets.UTF_8));
            z.closeEntry();
        }
        // Like the real installer: clobber launcher_profiles.json with its own profile.
        Files.write(new File(root, "launcher_profiles.json").toPath(),
            "{\"profiles\":{\"forge\":{\"name\":\"forge\",\"lastVersionId\":\"x\"}},\"version\":3}".getBytes(StandardCharsets.UTF_8));
        System.out.println("The client installed successfully");
    }
}
