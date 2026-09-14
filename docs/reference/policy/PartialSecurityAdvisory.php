<?php declare(strict_types=1);











namespace Composer\Advisory;

use Composer\Pcre\Preg;
use Composer\Semver\Constraint\Constraint;
use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\VersionParser;
use JsonSerializable;

class PartialSecurityAdvisory implements JsonSerializable
{




public $advisoryId;





public $packageName;





public $affectedVersions;





public static function create(string $packageName, array $data, VersionParser $parser): self
{
try {
$constraint = $parser->parseConstraints($data['affectedVersions']);
} catch (\UnexpectedValueException $e) {

try {
$affectedVersion = Preg::replace('{(^[>=<^~]*[\d.]+).*}', '$1', $data['affectedVersions']);
$constraint = $parser->parseConstraints($affectedVersion);
} catch (\UnexpectedValueException $e) {
$constraint = new Constraint('==', '0.0.0-invalid-version');
}
}

if (isset($data['title'], $data['sources'], $data['reportedAt'])) {
return new SecurityAdvisory($packageName, $data['advisoryId'], $constraint, $data['title'], $data['sources'], new \DateTimeImmutable($data['reportedAt'], new \DateTimeZone('UTC')), $data['cve'] ?? null, $data['link'] ?? null, $data['severity'] ?? null);
}

return new self($packageName, $data['advisoryId'], $constraint);
}

public function __construct(string $packageName, string $advisoryId, ConstraintInterface $affectedVersions)
{
$this->advisoryId = $advisoryId;
$this->packageName = $packageName;
$this->affectedVersions = $affectedVersions;
}




#[\ReturnTypeWillChange]
public function jsonSerialize()
{
$data = (array) $this;
$data['affectedVersions'] = $data['affectedVersions']->getPrettyString();

return $data;
}
}
