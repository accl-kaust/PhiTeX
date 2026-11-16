// A bibliography entry read off the page as a citation card: its authors,
// year, title and venue, in the styles papers use.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Viewer } from "../src/viewer.ts";

const asCited = (label: string, text: string) =>
  (Viewer as unknown as { asCited(l: string, t: string, at: [number, number]): { title: string; authors: string[]; venue?: string; year?: string } }).asCited(label, text, [0, 0]);

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
