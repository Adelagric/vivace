<?php declare(strict_types=1);











namespace Composer\DependencyResolver;











class RuleWatchGraph
{

protected $watchChains = [];













public function insert(RuleWatchNode $node): void
{
if ($node->getRule()->isAssertion()) {
return;
}

if (!$node->getRule() instanceof MultiConflictRule) {
foreach ([$node->watch1, $node->watch2] as $literal) {
if (!isset($this->watchChains[$literal])) {
$this->watchChains[$literal] = new RuleWatchChain;
}

$this->watchChains[$literal]->unshift($node);
}
} else {
foreach ($node->getRule()->getLiterals() as $literal) {
if (!isset($this->watchChains[$literal])) {
$this->watchChains[$literal] = new RuleWatchChain;
}

$this->watchChains[$literal]->unshift($node);
}
}
}
























public function propagateLiteral(int $decidedLiteral, int $level, Decisions $decisions): ?Rule
{



$literal = -$decidedLiteral;

if (!isset($this->watchChains[$literal])) {
return null;
}

$chain = $this->watchChains[$literal];

$chain->rewind();
while ($chain->valid()) {
$node = $chain->current();
if (!$node->getRule() instanceof MultiConflictRule) {
$otherWatch = $node->getOtherWatch($literal);

if (!$node->getRule()->isDisabled() && !$decisions->satisfy($otherWatch)) {
$ruleLiterals = $node->getRule()->getLiterals();

$alternativeLiteral = array_find($ruleLiterals, static function ($ruleLiteral) use ($literal, $otherWatch, $decisions): bool {
return $ruleLiteral !== $literal && $ruleLiteral !== $otherWatch && !$decisions->conflict($ruleLiteral);
});

if ($alternativeLiteral !== null) {
$this->moveWatch($literal, $alternativeLiteral, $node);
continue;
}

if ($decisions->conflict($otherWatch)) {
return $node->getRule();
}

$decisions->decide($otherWatch, $level, $node->getRule());
}
} else {
foreach ($node->getRule()->getLiterals() as $otherLiteral) {
if ($literal !== $otherLiteral && !$decisions->satisfy($otherLiteral)) {
if ($decisions->conflict($otherLiteral)) {
return $node->getRule();
}

$decisions->decide($otherLiteral, $level, $node->getRule());
}
}
}

$chain->next();
}

return null;
}










protected function moveWatch(int $fromLiteral, int $toLiteral, RuleWatchNode $node): void
{
if (!isset($this->watchChains[$toLiteral])) {
$this->watchChains[$toLiteral] = new RuleWatchChain;
}

$node->moveWatch($fromLiteral, $toLiteral);
$this->watchChains[$fromLiteral]->remove();
$this->watchChains[$toLiteral]->unshift($node);
}
}
