package ai.trogon.jetbrains

import org.junit.Assert.*
import org.junit.Test

class DiffApplierTest {

    // ── hasDiff ───────────────────────────────────────────────────────────────

    @Test
    fun `hasDiff returns false for plain text`() {
        assertFalse(DiffApplier.hasDiff("hello world\nno diff here"))
    }

    @Test
    fun `hasDiff returns true when unified diff header present`() {
        val diff = """
            --- a/src/main.rs
            +++ b/src/main.rs
            @@ -1 +1 @@
            -old
            +new
        """.trimIndent()
        assertTrue(DiffApplier.hasDiff(diff))
    }

    @Test
    fun `hasDiff returns false for minus-only line`() {
        assertFalse(DiffApplier.hasDiff("--- not a real diff"))
    }

    // ── applyHunks ────────────────────────────────────────────────────────────

    @Test
    fun `applyHunks replaces a line`() {
        val original = mutableListOf("line1", "old", "line3")
        val diff = listOf(
            "@@ -2,1 +2,1 @@",
            "-old",
            "+new",
        )
        val (result, _) = DiffApplier.applyHunks(original, diff, 0)
        assertEquals(listOf("line1", "new", "line3"), result)
    }

    @Test
    fun `applyHunks inserts a line`() {
        val original = mutableListOf("a", "b")
        val diff = listOf(
            "@@ -1,2 +1,3 @@",
            " a",
            "+inserted",
            " b",
        )
        val (result, _) = DiffApplier.applyHunks(original, diff, 0)
        assertEquals(listOf("a", "inserted", "b"), result)
    }

    @Test
    fun `applyHunks deletes a line`() {
        val original = mutableListOf("keep", "delete-me", "keep2")
        val diff = listOf(
            "@@ -1,3 +1,2 @@",
            " keep",
            "-delete-me",
            " keep2",
        )
        val (result, _) = DiffApplier.applyHunks(original, diff, 0)
        assertEquals(listOf("keep", "keep2"), result)
    }

    @Test
    fun `applyHunks context lines do not change content`() {
        val original = mutableListOf("ctx1", "target", "ctx2")
        val diff = listOf(
            "@@ -1,3 +1,3 @@",
            " ctx1",
            "-target",
            "+replaced",
            " ctx2",
        )
        val (result, _) = DiffApplier.applyHunks(original, diff, 0)
        assertEquals(listOf("ctx1", "replaced", "ctx2"), result)
    }

    @Test
    fun `applyHunks empty diff lines returns original unchanged`() {
        val original = mutableListOf("x", "y")
        val (result, _) = DiffApplier.applyHunks(original, emptyList(), 0)
        assertEquals(listOf("x", "y"), result)
    }

    @Test
    fun `applyHunks stops at next file header`() {
        val original = mutableListOf("line1")
        val diff = listOf(
            "@@ -1,1 +1,1 @@",
            "-line1",
            "+replaced",
            "--- a/next.kt",
            "+++ b/next.kt",
        )
        val (_, nextI) = DiffApplier.applyHunks(original, diff, 0)
        // Should stop before the next --- header
        assertEquals(3, nextI)
    }

    @Test
    fun `applyHunks handles multiple hunks in sequence`() {
        val original = mutableListOf("a", "b", "c", "d", "e")
        val diff = listOf(
            "@@ -1,1 +1,1 @@",
            "-a",
            "+A",
            "@@ -5,1 +5,1 @@",
            "-e",
            "+E",
        )
        val (result, _) = DiffApplier.applyHunks(original, diff, 0)
        assertEquals(listOf("A", "b", "c", "d", "E"), result)
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    @Test
    fun `parse returns empty map when readFile returns null`() {
        val diff = """
            --- a/missing.txt
            +++ b/missing.txt
            @@ -1,1 +1,1 @@
            -old
            +new
        """.trimIndent()
        val result = DiffApplier.parse(diff) { null }
        assertTrue(result.isEmpty())
    }

    @Test
    fun `parse applies changes and returns correct path`() {
        val original = mutableListOf("hello", "world")
        val diff = """
            --- a/test.txt
            +++ b/test.txt
            @@ -1,2 +1,2 @@
             hello
            -world
            +trogon
        """.trimIndent()
        val result = DiffApplier.parse(diff) { original.toList() }
        assertEquals(listOf("hello", "trogon"), result["test.txt"])
    }

    @Test
    fun `parse strips b slash prefix from path`() {
        val diff = """
            --- a/src/foo.kt
            +++ b/src/foo.kt
            @@ -1,1 +1,1 @@
            -x
            +y
        """.trimIndent()
        val result = DiffApplier.parse(diff) { listOf("x") }
        assertTrue("Expected key 'src/foo.kt', got: ${result.keys}", result.containsKey("src/foo.kt"))
    }

    @Test
    fun `parse handles diff embedded in prose`() {
        // Agent often wraps the diff in explanation text
        val diff = """
            Here are the changes I made:

            --- a/main.rs
            +++ b/main.rs
            @@ -1,1 +1,1 @@
            -fn old()
            +fn new()

            The function has been renamed.
        """.trimIndent()
        val result = DiffApplier.parse(diff) { listOf("fn old()") }
        assertEquals(listOf("fn new()"), result["main.rs"])
    }
}
