"""A stand-in for the `modal` package, just enough for the bridge to run against."""


class NotFoundError(Exception):
    pass


class _Reader:
    def __init__(self, text):
        self._text = text

    def read(self):
        return self._text


class _Process:
    stdout = _Reader("out")
    stderr = _Reader("err")

    def wait(self):
        return 0


class _Filesystem:
    def make_directory(self, path, create_parents=False):
        pass

    def write_bytes(self, data, path):
        pass


class _Tunnel:
    def __init__(self, url):
        self.host = "h.modal.host"
        self.port = 443
        self.url = url


class _Image:
    object_id = "im-1"

    @staticmethod
    def from_registry(reference):
        return _Image()

    @staticmethod
    def from_id(image_id):
        return _Image()


class _App:
    @staticmethod
    def lookup(name, create_if_missing=False):
        return name


class Sandbox:
    def __init__(self, object_id):
        self.object_id = object_id
        self.filesystem = _Filesystem()

    @staticmethod
    def create(**kwargs):
        return Sandbox("sb-1")

    @staticmethod
    def from_id(object_id):
        if object_id == "sb-missing":
            raise NotFoundError("no such sandbox")
        return Sandbox(object_id)

    def set_tags(self, tags):
        pass

    def poll(self):
        return 0 if self.object_id == "sb-done" else None

    def exec(self, *command):
        return _Process()

    def terminate(self):
        pass

    def tunnels(self, timeout=50):
        return {8080: _Tunnel("https://h.modal.host"), 9090: _Tunnel(lambda: "https://h.modal.host:9090")}

    def snapshot_filesystem(self, timeout, ttl):
        return _Image()


class Image(_Image):
    pass


App = _App
