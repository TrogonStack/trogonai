import fs from "node:fs";
import path from "node:path";
import matter from "gray-matter";
import { z } from "zod";

const AdrFrontmatter = z.object({
  number: z.string().regex(/^\d{4}$/),
  slug: z.string().min(1),
  status: z.enum(["draft", "accepted", "rejected", "superseded", "deprecated"]),
  date: z.coerce.date(),
});

type AdrRecord = {
  filePath: string;
  frontmatter: z.infer<typeof AdrFrontmatter>;
  title: string;
};

export async function readAdrRecords(rootDir: string): Promise<AdrRecord[]> {
  const adrDir = path.join(rootDir, "adr");
  const adrEntries = await fs.promises.readdir(adrDir, { withFileTypes: true });
  const files = adrEntries.filter(isFile).map(toFileName).filter(isAdrMarkdownFile).map(toAdrFilePath(adrDir));

  const records = files
    .sort()
    .map((filePath) => {
      const content = fs.readFileSync(filePath, "utf-8");
      const parsed = matter(content);
      const frontmatter = parseAdrFrontmatter(filePath, parsed.data);
      validateAdrPath(filePath, frontmatter);

      return {
        filePath,
        frontmatter,
        title: readTitle(filePath, parsed.content),
      };
    });

  validateUniqueAdrNumbers(records);

  return records.sort((left, right) => parseAdrNumber(left.frontmatter.number) - parseAdrNumber(right.frontmatter.number));
}

export function toAdrSidebarItem(record: AdrRecord) {
  return {
    text: record.title,
    link: `/adr/${record.frontmatter.number}-${record.frontmatter.slug}`,
  };
}

const GlossaryFrontmatter = z.object({
  term: z.string().min(1),
  section: z.string().min(1),
  order: z.number().int().nonnegative(),
});

type GlossaryRecord = {
  slug: string;
  frontmatter: z.infer<typeof GlossaryFrontmatter>;
};

const GLOSSARY_SECTION_ORDER = [
  "Repository and process",
  "Protocols and transports",
  "Event sourcing and the decider",
  "Agent execution model",
  "Messaging and storage infrastructure",
  "WebAssembly execution",
  "Wire contracts and serialization",
  "Identity, security, and multi-tenancy",
  "Observability",
];

export async function readGlossaryRecords(rootDir: string): Promise<GlossaryRecord[]> {
  const glossaryDir = path.join(rootDir, "glossary");
  const entries = await fs.promises.readdir(glossaryDir, { withFileTypes: true });
  const files = entries.filter(isFile).map(toFileName).filter(isGlossaryMarkdownFile);

  return files.map((fileName) => {
    const filePath = path.join(glossaryDir, fileName);
    const parsed = matter(fs.readFileSync(filePath, "utf-8"));

    return {
      slug: path.basename(fileName, ".md"),
      frontmatter: parseGlossaryFrontmatter(filePath, parsed.data),
    };
  });
}

export function toGlossarySidebarGroups(records: GlossaryRecord[]) {
  const recordsBySection = new Map<string, GlossaryRecord[]>();

  for (const record of records) {
    const section = record.frontmatter.section;
    const sectionRecords = recordsBySection.get(section) ?? [];
    sectionRecords.push(record);
    recordsBySection.set(section, sectionRecords);
  }

  const sections = [...recordsBySection.keys()].sort(compareGlossarySections);

  return sections.map((section) => ({
    text: section,
    collapsed: true,
    items: (recordsBySection.get(section) ?? [])
      .sort((left, right) => left.frontmatter.order - right.frontmatter.order)
      .map((record) => ({ text: record.frontmatter.term, link: `/glossary/${record.slug}` })),
  }));
}

function compareGlossarySections(left: string, right: string) {
  const leftIndex = GLOSSARY_SECTION_ORDER.indexOf(left);
  const rightIndex = GLOSSARY_SECTION_ORDER.indexOf(right);

  if (leftIndex === -1 || rightIndex === -1) {
    throw new Error(`Unknown glossary section. Add it to GLOSSARY_SECTION_ORDER: ${leftIndex === -1 ? left : right}`);
  }

  return leftIndex - rightIndex;
}

function parseGlossaryFrontmatter(filePath: string, data: unknown) {
  const result = GlossaryFrontmatter.safeParse(data);

  if (!result.success) {
    throw new Error(`${filePath}\n\n${z.prettifyError(result.error)}`);
  }

  return result.data;
}

function isGlossaryMarkdownFile(fileName: string) {
  return path.extname(fileName) === ".md" && fileName !== "index.md";
}

function parseAdrFrontmatter(filePath: string, data: unknown) {
  const result = AdrFrontmatter.safeParse(data);

  if (!result.success) {
    throw new Error(`${filePath}\n\n${z.prettifyError(result.error)}`);
  }

  return result.data;
}

function isFile(entry: fs.Dirent) {
  return entry.isFile();
}

function toFileName(entry: fs.Dirent) {
  return entry.name;
}

function isAdrMarkdownFile(fileName: string) {
  return path.extname(fileName) === ".md" && fileName !== "index.md";
}

function toAdrFilePath(adrDir: string) {
  return (fileName: string) => path.join(adrDir, fileName);
}

function validateAdrPath(filePath: string, frontmatter: z.infer<typeof AdrFrontmatter>) {
  const expectedFileName = `${frontmatter.number}-${frontmatter.slug}.md`;
  const actualFileName = path.basename(filePath);

  if (actualFileName !== expectedFileName) {
    throw new Error(`${filePath}\n\nExpected ADR filename to match frontmatter: ${expectedFileName}`);
  }
}

function validateUniqueAdrNumbers(records: AdrRecord[]) {
  const recordsByNumber = new Map<string, AdrRecord[]>();

  for (const record of records) {
    const number = record.frontmatter.number;
    const matchingRecords = recordsByNumber.get(number) ?? [];
    matchingRecords.push(record);
    recordsByNumber.set(number, matchingRecords);
  }

  const duplicates = [...recordsByNumber.entries()].filter(([, matchingRecords]) => matchingRecords.length > 1);

  if (duplicates.length === 0) {
    return;
  }

  const duplicateSummaries = duplicates.map(([number, matchingRecords]) => {
    const fileNames = matchingRecords.map((record) => path.basename(record.filePath)).join(", ");

    return `ADR#${number}: ${fileNames}`;
  });

  throw new Error(`ADR numbers must be unique.\n\n${duplicateSummaries.join("\n")}`);
}

function readTitle(filePath: string, content: string) {
  const title = content.match(/^#\s+(.+)$/m)?.[1];

  if (!title) {
    throw new Error(`${filePath}\n\nADR must have an H1 title.`);
  }

  return title;
}

function parseAdrNumber(number: string) {
  return Number.parseInt(number, 10);
}
