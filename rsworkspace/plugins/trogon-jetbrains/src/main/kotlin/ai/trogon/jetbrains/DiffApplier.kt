package ai.trogon.jetbrains

/**
 * Pure diff parsing and application logic — zero IDE dependencies.
 * Extracted from TrogonChatPanel for testability.
 */
object DiffApplier {

    /** Returns true if [text] contains a unified diff header. */
    fun hasDiff(text: String): Boolean =
        text.lineSequence().any { it.startsWith("+++ ") }

    /**
     * Parse a unified diff and return a map of relative file path → new line list.
     *
     * @param diff       the full unified diff text (may be embedded in prose)
     * @param readFile   callback that returns the current lines for a relative path,
     *                   or null if the file cannot be found
     */
    fun parse(diff: String, readFile: (String) -> List<String>?): Map<String, List<String>> {
        val result = mutableMapOf<String, List<String>>()
        val lines = diff.lines()
        var i = 0

        while (i < lines.size) {
            if (!lines[i].startsWith("--- ")) { i++; continue }
            if (i + 1 >= lines.size || !lines[i + 1].startsWith("+++ ")) { i++; continue }
            i += 2

            val relPath = lines[i - 1]          // the "+++ ..." line
                .removePrefix("+++ ")
                .removePrefix("b/")
                .trim()

            val original = readFile(relPath)?.toMutableList() ?: continue
            val (modified, nextI) = applyHunks(original, lines, i)
            i = nextI
            result[relPath] = modified
        }

        return result
    }

    /**
     * Apply all contiguous unified-diff hunks from [diffLines] starting at [startIdx]
     * against [original]. Returns the modified line list and the index where processing
     * stopped (next `--- ` header or end of input).
     */
    internal fun applyHunks(
        original: MutableList<String>,
        diffLines: List<String>,
        startIdx: Int,
    ): Pair<List<String>, Int> {
        val out = original.toMutableList()
        var i = startIdx
        var offset = 0

        while (i < diffLines.size) {
            val line = diffLines[i]
            // Stop at the next file header.
            if (line.startsWith("--- ") &&
                i + 1 < diffLines.size &&
                diffLines[i + 1].startsWith("+++ ")
            ) break

            if (!line.startsWith("@@")) { i++; continue }

            val m = Regex("""^@@ -(\d+)(?:,\d+)? \+\d+(?:,\d+)? @@""").find(line)
            if (m == null) { i++; continue }
            val oldStart = m.groupValues[1].toInt() - 1   // 0-based
            i++

            var pos = oldStart + offset

            while (i < diffLines.size) {
                val h = diffLines[i]
                when {
                    h.startsWith("@@") -> break
                    h.startsWith("--- ") &&
                        i + 1 < diffLines.size &&
                        diffLines[i + 1].startsWith("+++ ") -> break
                    h.startsWith("+") -> {
                        val content = h.substring(1)
                        if (pos <= out.size) out.add(pos, content) else out.add(content)
                        pos++; offset++
                    }
                    h.startsWith("-") -> {
                        if (pos < out.size) { out.removeAt(pos); offset-- }
                    }
                    h.startsWith(" ") -> pos++
                    // "\ No newline at end of file" and similar — skip
                }
                i++
            }
        }

        return Pair(out, i)
    }
}
