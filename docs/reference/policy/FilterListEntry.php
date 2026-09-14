<?php declare(strict_types=1);











namespace Composer\FilterList;

use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\VersionParser;





class FilterListEntry
{



public $packageName;




public $listName;




public $constraint;




public $url;




public $reason;




public $id;




public $source;

public function __construct(
string $packageName,
ConstraintInterface $constraint,
string $listName,
?string $url = null,
?string $reason = null,
?string $id = null,
?string $source = null
) {
$this->packageName = $packageName;
$this->listName = $listName;
$this->constraint = $constraint;
$this->url = $url;
$this->reason = $reason;
$this->id = $id;
$this->source = $source;
}




public static function create(string $listName, array $data, VersionParser $parser): self
{
$constraint = $parser->parseConstraints($data['constraint']);

return new self(
$data['package'],
$constraint,
$listName,
$data['url'] ?? null,
$data['reason'] ?? null,
$data['id'] ?? null,
$data['source'] ?? null
);
}
}
