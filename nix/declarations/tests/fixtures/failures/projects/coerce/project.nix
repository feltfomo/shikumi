# adds a function of Shikumi's to text, so the evaluator's message names the file that function is
# in, which the finding has to name without the path Shikumi is installed at.
{ shikumi, ... }:
shikumi.project {
  project = {
    summary = "made by " + shikumi.project;
    maintainers = [ "someone" ];
  };
}
