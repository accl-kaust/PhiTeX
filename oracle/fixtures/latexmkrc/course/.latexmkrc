# latexmk settings for the CS221P lab handouts.
# Puts common/tex/ (cs221p-lab.sty and the KAUST logos) on the TeX search
# path, so a lab builds from the repo root:
#     latexmk -pdf -outdir=/tmp/tex/lab1 lab1-setup/lab1.tex
# Lab folders have a small .latexmkrc that loads this one, so the same
# works from inside a lab folder:
#     cd lab1-setup && latexmk -pdf -outdir=/tmp/tex/lab1 lab1.tex
use File::Basename qw(dirname);
use Cwd qw(abs_path);
my $cs221p_root = dirname(abs_path(__FILE__));
ensure_path('TEXINPUTS', "$cs221p_root/common/tex//");
