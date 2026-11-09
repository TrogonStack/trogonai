from gate_errors import Code, GateError


class RustDocument:
    def __init__(self, document, aliases=None):
        self.document = document
        self.aliases = aliases or {}

    def normalize(self, value):
        return value

    def type(self, value):
        return self.normalize(value)

    def item(self, identifier):
        return self.document["index"][str(identifier)]
