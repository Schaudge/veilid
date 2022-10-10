# Terraform for Gitlab Runner

After having had trouble with my Gitlab Runner, I decided to put together a plan
for creating runners more automatically, thus this Terraform configuration.

This plan assumes running a Gitlab Runner, Docker Executor on a DigitalOcean
droplet. Running this plan requires an active DigitalOcean account, a configured
SSH key that will be installed on any created droplet, and a DigitalOcean
personal access token (PAT).

## Creating the runner

Before creating the runner, we run a `plan` to ensure we are creating the
droplet that we expect. First, we will export our access token as an environment
variable:

```shell
export DO_PAT="$(cat ~/.config/doctl/config.yaml | yq e '.access-token' -)"
```

Then we can run our plan:

```shell
terraform plan \
  -var "do_token=${DO_PAT}" \
  -var "pvt_key=$HOME/.ssh/id_rsa" \
  -var "ssh_key=$KEYNAME"
```

If the output is what was expected, we may now create the droplet:

```shell
terraform apply \
  -var "do_token=${DO_PAT}" \
  -var "pvt_key=$HOME/.ssh/id_rsa" \
  -var "ssh_key=$KEYNAME"
```

## Destroying the runner

```shell
terraform destroy \
  -var "do_token=${DO_PAT}" \
  -var "pvt_key=$HOME/.ssh/id_rsa" \
  -var "ssh_key=$KEYNAME"
```

**TODO**

Update the configuration to accept the runner registration token as a variable
and automatically self-register.
