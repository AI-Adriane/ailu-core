# Ailu PHP SDK

PHP FFI wrapper over `ailu-c-api`.

```php
<?php
require __DIR__ . "/src/Ailu.php";

$ailu = Ailu::load(getenv("AILU_C_API_LIB"));
echo $ailu->engineVersion() . PHP_EOL;
echo $ailu->listComponentsJson() . PHP_EOL;
```

PHP must have the FFI extension enabled.
