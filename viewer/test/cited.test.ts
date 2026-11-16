// A bibliography entry read off the page as a citation card: its authors,
// year, title and venue, in the styles papers use.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Viewer } from "../src/viewer.ts";

const asCited = (label: string, text: string) =>
  (Viewer as unknown as { asCited(l: string, t: string, at: [number, number]): { title: string; authors: string[]; venue?: string; year?: string; doi?: string; url?: string } }).asCited(label, text, [0, 0]);

test("ACM's year sentence is no title: Authors. 2025. Title. Venue.", () => {
  const c = asCited("9", "Heng Cao, Zhipeng Wu, Dejian Li, Yue Ma, and Lin Zhou. 2025. Accelerating Control Flow on Reconfigurable Arrays. In Proceedings of the 52nd Annual International Symposium on Computer Architecture. 1–14.");
  assert.equal(c.title, "Accelerating Control Flow on Reconfigurable Arrays");
  assert.equal(c.year, "2025");
  assert.deepEqual(c.authors.slice(0, 2), ["Heng Cao", "Zhipeng Wu"]);
  assert.match(c.venue ?? "", /^In Proceedings of the 52nd/);
});

test("IEEE and plain styles keep their title: Authors, Title, Venue, Year.", () => {
  const c = asCited("3", "D. E. Knuth and M. F. Plass. Breaking paragraphs into lines. Software: Practice and Experience, 11(11):1119–1184, 1981.");
  assert.equal(c.title, "Breaking paragraphs into lines");
  assert.equal(c.year, "1981");
});

test("a DOI broken across lines is one DOI, a link of its own, and no longer in the venue", () => {
  const c = asCited("5", "Thilini Kaushalya Bandara, Dhananjaya Wijerathne, Tulika Mitra and Li-Shiuan Peh. 2022. REVAMP: A Systematic Framework for Heterogeneous CGRA Realization. In Proceedings of the 27th ACM International Conference on Architectural Support for Programming Languages and Operating Systems (Lausanne Switzerland, 2022-02-28). ACM, New York, NY, USA, 918–932. doi:10. 1145/3503222.3507772");
  assert.equal(c.doi, "10.1145/3503222.3507772");
  assert.equal(c.title, "REVAMP: A Systematic Framework for Heterogeneous CGRA Realization");
  assert.doesNotMatch(c.venue ?? "", /doi|1145/);
  assert.match(c.venue ?? "", /918–932$/);
});

test("a DOI broken inside its suffix, and one given as a doi.org address", () => {
  assert.equal(asCited("1", "A. Author. A title. Some Journal, 2020. doi:10.1109/TC. 2020.3012345.").doi, "10.1109/TC.2020.3012345");
  const c = asCited("2", "B. Author. Another title. Venue, 2019. https://doi. org/10.1145/1234567.7654321");
  assert.equal(c.doi, "10.1145/1234567.7654321");
  assert.doesNotMatch(c.venue ?? "", /doi/);
});

test("an address broken after a slash is one address", () => {
  const c = asCited("4", "C. Author. Tool. Online, 2024. https://github.com/ example/repo (visited 2024).");
  assert.equal(c.url, "https://github.com/example/repo");
});
